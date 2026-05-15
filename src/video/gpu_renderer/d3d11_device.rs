//! D3D11 デバイス + ビデオプロセッサ + 共有 RGBA 出力テクスチャ管理。
//!
//! このモジュールは D3D11 単独で完結し、wgpu / egui には依存しない。FFmpeg の
//! `hw_device_ctx` に渡せる `ID3D11Device` と、NV12 → RGBA 変換を実行する
//! `ID3D11VideoProcessor` を管理する。
//!
//! ## 共有テクスチャの作り方
//! wgpu (d3d12) と D3D11 の間でフレームを受け渡すには **NT shared handle** を使う。
//! D3D11 側で `D3D11_RESOURCE_MISC_SHARED_NTHANDLE | D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX`
//! でテクスチャを作り、`IDXGIResource1::CreateSharedHandle` で HANDLE を取り、
//! それを `ID3D12Device::OpenSharedHandle` で D3D12 側で開く。Keyed Mutex は
//! 書き込み (D3D11) と読み取り (D3D12) の同期に使う。
//!
//! ## エラー方針
//! D3D11 / VideoDevice の作成は **失敗してもパニックしない**。失敗時は呼び出し側に
//! `Err` を返し、呼び出し側は SW (CPU readback + swscale) 経路にフォールバックする。

use std::cell::UnsafeCell;
use std::ptr;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};

use windows::Win32::Foundation::{HANDLE, HMODULE};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_CREATE_DEVICE_FLAG, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_FENCE_FLAG_SHARED,
    D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX, D3D11_RESOURCE_MISC_SHARED_NTHANDLE, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
    D3D11_VIDEO_PROCESSOR_CONTENT_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_STREAM,
    D3D11_VIDEO_USAGE_PLAYBACK_NORMAL, D3D11_VPIV_DIMENSION_TEXTURE2D,
    D3D11_VPOV_DIMENSION_TEXTURE2D, D3D11CreateDevice, ID3D11Device, ID3D11Device5,
    ID3D11DeviceContext, ID3D11DeviceContext4, ID3D11Fence, ID3D11Texture2D, ID3D11VideoContext,
    ID3D11VideoContext1, ID3D11VideoDevice, ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator,
    ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709, DXGI_COLOR_SPACE_TYPE,
    DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709, DXGI_COLOR_SPACE_YCBCR_STUDIO_G2084_LEFT_P2020,
    DXGI_COLOR_SPACE_YCBCR_STUDIO_GHLG_TOPLEFT_P2020, DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM,
    DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{IDXGIKeyedMutex, IDXGIResource1};
use windows::Win32::System::Threading::{
    AcquireSRWLockExclusive, ReleaseSRWLockExclusive, SRWLOCK,
};
use windows::core::Interface;

/// 出力テクスチャのリングバッファサイズ。decoder が書き → UI が読む間
/// に少なくとも 2 枚 in-flight を許す。
/// 共有出力テクスチャ pool の上限スロット数。各 slot は 1 枚の D3D11 ID3D11Texture2D
/// (NT shared) を保持し、4K BGRA で約 32 MB / 1080p で約 8 MB を adapter memory に使う。
///
/// 2026-05-15 (Codex 助言): 旧 24 → 16 に縮小。fast-swap 連発時に解像度違いで
/// evict + grow が大量に走り、`shared_texture_cache` (presenter 側 cap=8) や
/// `retired_video_surfaces` (depth=1) と合わせて adapter memory が枯渇 →
/// `wgpu Out of Memory` panic を発生させていた。16 にすることで 4K で
/// pool 単独最大 ~512 MB を上限化する。`source.queue` (`MAX_NATIVE_SOURCE_QUEUE=8`)
/// + presenter 側で消費されるので 16 でも slot 枯渇は起きにくいが、pool exhausted
/// 時は `GpuVideoError::ResourcePressure` で frame drop に落とす (CPU readback fallback
/// は **しない** = pressure を悪化させる)。
const OUTPUT_RING_SIZE: usize = 16;

/// 1 フレームの GPU 出力。
///
/// 同期は **D3D11/D3D12 共有 fence** で行う:
/// - decoder thread で `CopyResource` 完了後に `ID3D11DeviceContext4::Signal(fence, value)`
/// - UI thread (egui_wgpu) は wgpu の DX12 command queue で `Wait(fence, value)` してから
///   このフレームを sample する
///
/// SAFETY: `shared_handle` は NT HANDLE で `*mut c_void` 相当だが、本構造体は
/// channel 越しにスレッドを跨いで運ぶ必要があるため `Send` を unsafe impl する。
/// HANDLE 自体は OS 内部のテーブル参照なので、複数スレッドから順次使うことは安全。
pub struct D3d11Frame {
    pub width: u32,
    pub height: u32,
    /// pts (秒)。decoder が書く。
    pub pts_secs: f64,
    /// シーク世代。
    pub seek_serial: u64,
    /// NT shared handle (テクスチャ用、フレーム毎に新規)。wgpu 側で `OpenSharedHandle` する。
    /// `Drop` で `CloseHandle` する責任は所有者にある。
    pub shared_handle: HANDLE,
    pub close_shared_handle_on_drop: bool,
    pub shared_output_in_use: Option<Arc<AtomicBool>>,
    pub shared_output_notify: Option<Arc<Condvar>>,
    pub shared_output_keyed_mutex: Option<IDXGIKeyedMutex>,
    pub shared_output_released_to_reader: Option<Arc<AtomicBool>>,
    /// Display texture format selector for legacy import/cache paths.
    ///
    /// The current display path intentionally does not expose HDR output, so
    /// decoder-produced GPU frames are normalized to BGRA8 even for P010/P016
    /// sources and this flag is false.
    pub ten_bit: bool,
    /// このフレームの GPU 完了に対応する fence 値。`ID3D11DeviceContext4::Signal` で
    /// この値まで進めてあるので、wgpu 側は `Wait(fence, fence_value)` してから
    /// テクスチャを sample すれば 中身が確実に書き込まれている。
    pub fence_value: u64,
    /// fence の NT shared handle。wgpu 側でこれを `ID3D12Fence` として開くが、
    /// `GpuVideoDevice` の寿命中は同じ値が来るので、wgpu 側でキャッシュ判定キーに使う。
    /// **HANDLE 自体の所有権は `GpuVideoDevice` にあり、本フレームは値を借りているだけ。
    /// `Drop` で close してはいけない**。
    pub fence_shared_handle: HANDLE,
    /// プロセス内ユニークな fence 世代 ID。HANDLE 値だけだと kernel が値を
    /// 再利用したときに stale な D3D12 fence をキャッシュしたまま使ってしまうため、
    /// この値で再 open 判定する (Codex P1)。
    pub fence_gen: u64,
    /// この出力テクスチャ slot の世代 ID。fence と同様、NT shared handle の値は
    /// slot を evict した後に OS が再利用しうるため、handle 値だけでは presenter の
    /// `shared_texture_cache` が前動画の stale テクスチャを返してしまう。`shared_handle`
    /// と組で presenter のキャッシュキーにする。
    pub shared_texture_gen: u64,
}

// HANDLE は OS のリソース ID 相当 (= 単純な i64 値) で、所有権を移動する分には
// thread-safe。Sync は付けない (= 同時参照は許さない)。
unsafe impl Send for D3d11Frame {}

impl D3d11Frame {
    /// Return an unpresented pooled output texture to the producer-side pool.
    ///
    /// Do **not** call `AcquireSync(1)` here. NVIDIA's D3D11 driver can block
    /// inside keyed-mutex acquire even for short/zero timeouts, and this method
    /// is called from seek/close/drop paths on latency-sensitive threads. Leave
    /// `released_to_reader=true` in place and let `acquire_shared_output`
    /// recover key=1 on the producer thread when the slot is reused.
    pub fn reset_unpresented_shared_output(&mut self) {
        if self.shared_output_keyed_mutex.is_none()
            && self.shared_output_released_to_reader.is_none()
        {
            return;
        };
        crate::perf::event(
            "video",
            "shared_output_unpresented_reset_deferred",
            None,
            0,
            &[
                (
                    "shared_handle",
                    serde_json::Value::from(self.shared_handle.0 as usize as u64),
                ),
                (
                    "released_to_reader",
                    serde_json::Value::from(
                        self.shared_output_released_to_reader
                            .as_ref()
                            .is_some_and(|released| released.load(Ordering::Acquire)),
                    ),
                ),
            ],
        );
    }
}

impl Drop for D3d11Frame {
    fn drop(&mut self) {
        // NT shared HANDLE は CreateSharedHandle で取得しているので明示的に
        // CloseHandle で解放しないと毎フレームのリークになる。
        // wgpu 側 (D3D12 OpenSharedHandle) は HANDLE 値を内部複製しているので、
        // ここで close しても D3D12 リソースは生存する。
        if self.close_shared_handle_on_drop && !self.shared_handle.is_invalid() {
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(self.shared_handle);
            }
        }
        if let Some(in_use) = self.shared_output_in_use.take() {
            in_use.store(false, Ordering::Release);
        }
        if let Some(notify) = self.shared_output_notify.take() {
            notify.notify_one();
        }
    }
}

/// `acquire_shared_output` で取得した slot を、`blit_nv12_to_rgba` の途中で
/// 早期 return (= `?`) されても確実に解放するための RAII guard。
///
/// 2026-05-15 (Codex P1): 旧コードは `acquire_shared_output` で `in_use=true` + keyed
/// mutex AcquireSync(0) した後、CreateInputView / CreateOutputView / VideoProcessorBlt /
/// Signal のいずれかが `?` で失敗すると `D3d11Frame` が作られないため `in_use=false`
/// が呼ばれず、**slot が永久占有 (= LEAK)** する状態だった。`ResourcePressure` を
/// frame drop に倒したことで失敗パスが日常的に踏まれるようになり、pool slot が枯渇する
/// 度に新しい decoder が「準備中」のまま固着する症状が観測された。
///
/// 本 guard は acquire 直後に作り、成功して `BlitOutput` を返す直前に `disarm()` を
/// 呼ぶ。途中で `?` 早期 return すると `Drop` で slot を返却する。状態:
///   - `holding_write_key=true` (= ReleaseSync(1) 前): `ReleaseSync(0)` で write key を戻し、
///     `in_use=false` + notify する。
///   - `holding_write_key=false` (= ReleaseSync(1) 成功後、Signal 失敗等): write key は
///     既に reader 側 (`released_to_reader=true`)。`ReleaseSync` は呼ばず in_use のみ戻す。
///     次回 acquire で `recover_shared_output_keyed_mutex` 経由で key=1 を取り戻す。
struct SharedOutputSlotGuard {
    in_use: Option<Arc<AtomicBool>>,
    notify: Option<Arc<Condvar>>,
    keyed_mutex: Option<IDXGIKeyedMutex>,
    /// true = まだ write key (=0) を保有中。false = ReleaseSync(1) 済 (reader に渡した)。
    holding_write_key: bool,
    /// false にすると Drop で何もしない (= 成功 handoff 用)。
    armed: bool,
}

impl SharedOutputSlotGuard {
    fn new(
        in_use: Option<Arc<AtomicBool>>,
        notify: Option<Arc<Condvar>>,
        keyed_mutex: Option<IDXGIKeyedMutex>,
        had_write_key: bool,
    ) -> Self {
        // pool 由来 slot のみ guard 対象 (= in_use が Some)。create_shared_output の
        // 非 pool 経路 (= acquire 失敗 / km cast 失敗時) は in_use=None で armed=false
        // 相当の動作にする。
        let armed = in_use.is_some();
        Self {
            in_use,
            notify,
            keyed_mutex,
            holding_write_key: had_write_key,
            armed,
        }
    }

    /// ReleaseSync(1) が成功した直後に呼ぶ。write key は reader 側に渡ったので、
    /// 以降の Drop では ReleaseSync(0) を呼ばない。
    fn mark_released_to_reader(&mut self) {
        self.holding_write_key = false;
    }

    /// `BlitOutput` を返す直前に呼ぶ。slot ownership が `D3d11Frame` に移ったので
    /// Drop では何もしない。
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for SharedOutputSlotGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if self.holding_write_key {
            if let Some(km) = self.keyed_mutex.as_ref() {
                // 失敗時の writer-side release: 次の acquire が AcquireSync(0) で取れる
                // 状態に戻す。失敗しても致命ではない (= 次の acquire は
                // recover_shared_output_keyed_mutex で取り戻せる) のでログだけ。
                unsafe {
                    if let Err(e) = km.ReleaseSync(0) {
                        crate::logger::log(format!(
                            "[shared-output] guard ReleaseSync(0) failed: {e:?}"
                        ));
                    }
                }
            }
        }
        if let Some(in_use) = self.in_use.as_ref() {
            in_use.store(false, Ordering::Release);
        }
        if let Some(notify) = self.notify.as_ref() {
            notify.notify_one();
        }
        crate::logger::log(format!(
            "[shared-output] slot guard released slot without handoff: holding_write_key={}",
            self.holding_write_key
        ));
        if crate::perf::is_enabled() {
            crate::perf::event(
                "video",
                "shared_output_drop_unfinished",
                None,
                0,
                &[(
                    "holding_write_key",
                    serde_json::Value::from(self.holding_write_key),
                )],
            );
        }
    }
}

#[derive(Debug)]
pub enum GpuVideoError {
    DeviceCreate(String),
    NoVideoDevice,
    EnumeratorCreate(String),
    ProcessorCreate(String),
    TextureCreate(String),
    SharedHandle(String),
    Blt(String),
    /// GPU リソース圧迫 (shared output pool 枯渇、`E_OUTOFMEMORY=0x8007000E`、
    /// `D3D11_ERROR_TOO_MANY_UNIQUE_VIEW_OBJECTS` など回復不能な resource pressure 系
    /// エラー、2026-05-15 追加)。decoder 側はこのバリアントを受けたら **CPU readback
    /// fallback してはいけない** (= 内部 swscale + GPU upload で adapter memory を
    /// さらに圧迫し OOM 連鎖を加速させる)。frame drop / 短い backoff に倒す。
    ResourcePressure(String),
}

impl GpuVideoError {
    /// `ResourcePressure` バリアントか判定。decoder 側で CPU fallback を抑止する用途。
    pub fn is_resource_pressure(&self) -> bool {
        matches!(self, Self::ResourcePressure(_))
    }

    /// `windows::core::Error` の HRESULT が GPU resource pressure 系か判定。
    ///   - `E_OUTOFMEMORY (0x8007000E)` = adapter / system memory 不足
    ///   - `D3D11_ERROR_TOO_MANY_UNIQUE_VIEW_OBJECTS (0x887C0003)` = 内部 view object pool 枯渇
    ///   - `D3D11_ERROR_TOO_MANY_UNIQUE_STATE_OBJECTS (0x887C0001)` = state object pool 枯渇
    ///   - `DXGI_ERROR_DEVICE_REMOVED (0x887A0005)` は別軸 (device lost) なので含めない
    ///
    /// 2026-05-15: 旧コードの `0x88790007` は誤値 (Codex P2 review)。`windows::Win32::
    /// Graphics::Direct3D11::D3D11_ERROR_TOO_MANY_UNIQUE_VIEW_OBJECTS` の実際の HRESULT は
    /// `0x887C0003`。
    pub fn hresult_is_resource_pressure(err: &windows::core::Error) -> bool {
        let code = err.code().0 as u32;
        matches!(code, 0x8007000E | 0x887C0001 | 0x887C0003)
    }

    /// `windows::core::Error` を `ResourcePressure` か `Blt` に振り分ける helper
    /// (CreateInputView / CreateOutputView / Blt 用)。
    fn from_blt_hresult(label: &str, err: windows::core::Error) -> Self {
        if GpuVideoError::hresult_is_resource_pressure(&err) {
            Self::ResourcePressure(format!("{label}: {err:?}"))
        } else {
            Self::Blt(format!("{label}: {err:?}"))
        }
    }
}

impl std::fmt::Display for GpuVideoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeviceCreate(s) => write!(f, "D3D11CreateDevice failed: {s}"),
            Self::NoVideoDevice => write!(f, "ID3D11VideoDevice not supported on this adapter"),
            Self::EnumeratorCreate(s) => write!(f, "CreateVideoProcessorEnumerator failed: {s}"),
            Self::ProcessorCreate(s) => write!(f, "CreateVideoProcessor failed: {s}"),
            Self::TextureCreate(s) => write!(f, "CreateTexture2D failed: {s}"),
            Self::SharedHandle(s) => write!(f, "CreateSharedHandle failed: {s}"),
            Self::Blt(s) => write!(f, "VideoProcessorBlt failed: {s}"),
            Self::ResourcePressure(s) => write!(f, "GPU resource pressure: {s}"),
        }
    }
}

impl std::error::Error for GpuVideoError {}

/// HW デコード + GPU NV12→RGBA 変換用の共有 D3D11 デバイス。
///
/// FFmpeg の `AVCodecContext.hw_device_ctx` にこのデバイスを渡すと、HW デコーダの
/// 出力 NV12 テクスチャが本デバイス上に生成され、`VideoProcessorBlt` で同じデバイス
/// 内のリング出力テクスチャに RGBA 変換できる (= GPU→CPU 転送なし)。
#[allow(dead_code)]
pub struct GpuVideoDevice {
    device: ID3D11Device,
    /// SAFETY: ID3D11DeviceContext は **Send/Sync 安全ではない**。本構造体は全体を
    /// `Mutex` で囲む前提で外から使い、内部では context を直接共有する。
    context: ID3D11DeviceContext,
    /// `Signal(fence, value)` を呼ぶための ID3D11DeviceContext4 view。`context` と同一
    /// オブジェクトを cast 経由で持っているだけ。
    context4: ID3D11DeviceContext4,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    /// `VideoProcessorSetStreamColorSpace1` / `VideoProcessorSetOutputColorSpace1` を
    /// 呼ぶための ID3D11VideoContext1 view (= `video_context` と同一オブジェクトの cast)。
    video_context1: ID3D11VideoContext1,
    /// processor + enumerator のキャッシュ。同一 (in_size, out_size, format) なら使い回す。
    /// `Mutex` であること自体が correctness 要件: 動画切替時は旧 decoder thread が
    /// detached のまま生きており、新 decoder thread と同じ `Arc<GpuVideoDevice>` を
    /// 共有して同時に `blit` を呼ぶ。`RefCell` だと並行 borrow で panic する
    /// (2026-05-14 実害: ホイール連続切替で「デコード開始中」固着)。`blit` は
    /// このロックを処理全体で保持するので、結果的に並行 blit 全体が直列化される。
    processor_cache: Mutex<Option<ProcessorState>>,
    shared_output_pool: Mutex<Vec<SharedOutputSlot>>,
    shared_output_pool_wait: Mutex<()>,
    shared_output_pool_cv: Arc<Condvar>,
    /// CopyResource 完了の signaling 用。device 寿命中 1 個。各フレームは
    /// `next_fence_value.fetch_add(1)` で得た値で `Signal(fence, value)` する。
    fence: ID3D11Fence,
    /// `fence` の NT shared handle (D3D12 側で `OpenSharedHandle` するため)。
    /// `Drop` で `CloseHandle` する責任は本構造体にある。
    fence_shared_handle: HANDLE,
    /// 次に Signal する fence 値 (1, 2, 3, ... と単調増加)。
    next_fence_value: std::sync::atomic::AtomicU64,
    /// プロセスグローバルにユニークな fence の世代 ID。HANDLE 値だけだと
    /// kernel が値を再利用したときに stale な D3D12 fence をキャッシュ
    /// したまま使ってしまう (= sync race 復活、または永久 Wait)。
    /// wgpu 側は `fence_gen` で再 open 判定する。
    fence_gen: u64,
    /// `ID3D11DeviceContext` / `ID3D11VideoContext` の同時呼び出しを直列化するための
    /// SRWLOCK。**FFmpeg の D3D11VA decode (`avcodec_send_packet` / `receive_frame` が
    /// 内部で device context を触る) と `blit_nv12_to_rgba` の immediate context 操作が
    /// 同じ `ID3D11DeviceContext` を共有している**ため、両者を直列化しないと driver 内で
    /// hard-stuck する (fast-swap 連射で実害。本ファイル冒頭の `LIVE_VIDEO_DECODE_THREADS`
    /// コメント参照)。`processor_cache: Mutex` は blit 同士しか直列化せず FFmpeg 側を
    /// 守れない。
    ///
    /// FFmpeg には `AVD3D11VADeviceContext.lock/unlock` callback でこの SRWLOCK を
    /// 渡し (`ffmpeg_d3d11.rs`)、`blit_nv12_to_rgba` の context 操作区間も `lock_d3d_context`
    /// で同じ SRWLOCK を握る。C callback 跨ぎで lock/unlock を別々に呼ぶ必要があるため
    /// RAII guard を返せない `SRWLOCK` を使う。`SRWLOCK::default()` (= 全 0) が
    /// `SRWLOCK_INIT` で有効な初期状態。`&self` 経由で内部可変するため `UnsafeCell` で包む
    /// (`GpuVideoDevice` の `unsafe impl Sync` がこの共有可変を保証する)。
    d3d_lock: UnsafeCell<SRWLOCK>,
}

/// 入力動画の色空間ヒント。FFmpeg の transfer characteristic から決定し、
/// `VideoProcessorSetStreamColorSpace1` に渡す DXGI_COLOR_SPACE_TYPE を選ぶ。
/// SDR を default とし、HDR PQ / HDR HLG だけ別扱い (Codex Medium)。
#[derive(Clone, Copy, Debug)]
pub enum VideoColorHint {
    /// BT.709 SDR studio range (8-bit NV12 や SDR 10-bit P010 など)。
    Sdr,
    /// BT.2020 PQ (SMPTE2084) HDR。
    HdrPq,
    /// BT.2020 HLG (ARIB STD B67) HDR。
    HdrHlg,
}

/// `blit_nv12_to_rgba` の戻り値。
/// - `output_texture` は呼び出し側でこのフレームの寿命中保持する責任を負う
///   (= drop すると D3D11 側のテクスチャ解放、ただし NT shared handle は別命数管理なので
///   D3D12 側からは引き続き参照可能)
/// - `shared_handle` is either owned by `D3d11Frame` or by the shared-output pool
/// - `fence_value` を `D3d11Frame` に乗せて wgpu 側 `Wait(fence, fence_value)` に渡す
pub struct BlitOutput {
    pub output_texture: ID3D11Texture2D,
    pub shared_handle: HANDLE,
    pub close_shared_handle_on_drop: bool,
    pub shared_output_in_use: Option<Arc<AtomicBool>>,
    pub shared_output_notify: Option<Arc<Condvar>>,
    pub shared_output_keyed_mutex: Option<IDXGIKeyedMutex>,
    pub shared_output_released_to_reader: Option<Arc<AtomicBool>>,
    pub fence_value: u64,
    /// この出力テクスチャ slot の世代 ID (`SharedOutputSlot::texture_gen`)。`D3d11Frame`
    /// に載せて presenter まで運び、`shared_texture_cache` のキーに含めることで
    /// NT shared handle 値の再利用による stale テクスチャ参照を防ぐ。
    pub shared_texture_gen: u64,
}

struct SharedOutputSlot {
    tex: ID3D11Texture2D,
    shared_handle: HANDLE,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
    in_use: Arc<AtomicBool>,
    released_to_reader: Arc<AtomicBool>,
    /// プロセス内ユニークな世代 ID。slot 生成ごとに採番する。NT shared handle の値は
    /// slot を evict (= `CloseHandle`) した後に OS が別 slot 用へ再利用しうるため、
    /// handle 値だけだと native presenter 側の `shared_texture_cache` が stale な
    /// テクスチャを返してしまう (= 動画切替で前動画のフレームが 1 枚混入する)。
    /// `fence_gen` と同じ思想の identity で、これを `D3d11Frame` 経由で presenter まで
    /// 運び、キャッシュキーに含める。
    texture_gen: u64,
}

impl Drop for SharedOutputSlot {
    fn drop(&mut self) {
        if !self.shared_handle.is_invalid() {
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(self.shared_handle);
            }
        }
    }
}

struct ProcessorState {
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    in_w: u32,
    in_h: u32,
    in_format: DXGI_FORMAT,
    out_w: u32,
    out_h: u32,
    out_format: DXGI_FORMAT,
    /// `ContentDesc.InputFrameRate` に渡した実 fps。違う fps の動画に切り替わった
    /// ときに processor を作り直すためキャッシュキーに含める (Codex Medium)。
    /// 0/0 はフォールバック判定後の値 (60/1 等) を保持。
    fps_num: u32,
    fps_den: u32,
}

/// `GpuVideoDevice::lock_d3d_context` が返す RAII guard。Drop で SRWLOCK を release する。
struct D3dContextGuard<'a> {
    lock: *mut SRWLOCK,
    _marker: std::marker::PhantomData<&'a GpuVideoDevice>,
}

impl Drop for D3dContextGuard<'_> {
    fn drop(&mut self) {
        // SAFETY: `lock` は `GpuVideoDevice::d3d_lock` の生ポインタ。guard の寿命は
        // `&self` 借用 (`PhantomData`) に縛られており、その間 `GpuVideoDevice` は生存。
        unsafe {
            ReleaseSRWLockExclusive(self.lock);
        }
    }
}

impl GpuVideoDevice {
    /// D3D11 デバイス + ビデオデバイスを作成する。失敗時はフォールバックを呼び出し側で
    /// 処理する想定。
    pub fn new() -> Result<Arc<Self>, GpuVideoError> {
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        let mut feature_level: D3D_FEATURE_LEVEL = D3D_FEATURE_LEVEL::default();
        let feature_levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];

        // BGRA_SUPPORT: wgpu 側で BGRA8UnormSrgb を使うため
        // VIDEO_SUPPORT: ID3D11VideoDevice / ID3D11VideoContext の取得に必要
        let flags = D3D11_CREATE_DEVICE_FLAG(
            D3D11_CREATE_DEVICE_BGRA_SUPPORT.0 | D3D11_CREATE_DEVICE_VIDEO_SUPPORT.0,
        );

        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                flags,
                Some(&feature_levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                Some(&mut feature_level),
                Some(&mut context),
            )
            .map_err(|e| GpuVideoError::DeviceCreate(format!("{e:?}")))?;
        }
        let device = device.ok_or_else(|| {
            GpuVideoError::DeviceCreate("D3D11CreateDevice returned null device".into())
        })?;
        let context = context.ok_or_else(|| {
            GpuVideoError::DeviceCreate("D3D11CreateDevice returned null context".into())
        })?;

        let video_device: ID3D11VideoDevice =
            device.cast().map_err(|_| GpuVideoError::NoVideoDevice)?;
        let video_context: ID3D11VideoContext =
            context.cast().map_err(|_| GpuVideoError::NoVideoDevice)?;
        let video_context1: ID3D11VideoContext1 = video_context
            .cast()
            .map_err(|e| GpuVideoError::DeviceCreate(format!("ID3D11VideoContext1 cast: {e:?}")))?;

        crate::logger::log(format!(
            "GpuVideoDevice: created (feature_level=0x{:X})",
            feature_level.0
        ));

        // 共有 fence を作成。ID3D11Device5 (= D3D11.4) で初めて利用可能だが、
        // Windows 10 1809 以降の更新済み環境では存在する。失敗したら呼び出し側で
        // SW フォールバックされるよう Err を返す。
        let device5: ID3D11Device5 = device
            .cast()
            .map_err(|e| GpuVideoError::DeviceCreate(format!("cast ID3D11Device5: {e:?}")))?;
        let context4: ID3D11DeviceContext4 = context.cast().map_err(|e| {
            GpuVideoError::DeviceCreate(format!("cast ID3D11DeviceContext4: {e:?}"))
        })?;
        let mut fence_opt: Option<ID3D11Fence> = None;
        unsafe {
            device5
                .CreateFence(0, D3D11_FENCE_FLAG_SHARED, &mut fence_opt)
                .map_err(|e| GpuVideoError::DeviceCreate(format!("CreateFence: {e:?}")))?;
        }
        let fence = fence_opt
            .ok_or_else(|| GpuVideoError::DeviceCreate("CreateFence returned null".into()))?;
        let fence_shared_handle = unsafe {
            fence
                .CreateSharedHandle(
                    None,
                    windows::Win32::Foundation::GENERIC_ALL.0,
                    windows::core::PCWSTR::null(),
                )
                .map_err(|e| {
                    GpuVideoError::DeviceCreate(format!("Fence CreateSharedHandle: {e:?}"))
                })?
        };

        // プロセス内ユニーク世代 ID。0 は予約 (= 未開封キャッシュ判定で使う)、
        // GpuVideoDevice の生成ごとに 1, 2, 3, ... と進む。HANDLE 値は kernel が
        // 再利用しうるので別軸の identity が必要 (Codex P1)。
        static NEXT_FENCE_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let fence_gen = NEXT_FENCE_GEN.fetch_add(1, std::sync::atomic::Ordering::AcqRel);

        Ok(Arc::new(Self {
            device,
            context,
            context4,
            video_device,
            video_context,
            video_context1,
            processor_cache: Mutex::new(None),
            shared_output_pool: Mutex::new(Vec::new()),
            shared_output_pool_wait: Mutex::new(()),
            shared_output_pool_cv: Arc::new(Condvar::new()),
            fence,
            fence_shared_handle,
            next_fence_value: std::sync::atomic::AtomicU64::new(0),
            fence_gen,
            d3d_lock: UnsafeCell::new(SRWLOCK::default()),
        }))
    }

    pub fn fence_shared_handle(&self) -> HANDLE {
        self.fence_shared_handle
    }

    pub fn fence_gen(&self) -> u64 {
        self.fence_gen
    }

    pub fn raw_device(&self) -> &ID3D11Device {
        &self.device
    }

    pub fn raw_context(&self) -> &ID3D11DeviceContext {
        &self.context
    }

    /// D3D11VA context 直列化用 SRWLOCK の生ポインタ。`ffmpeg_d3d11.rs` が
    /// `AVD3D11VADeviceContext.lock_ctx` に渡す。`GpuVideoDevice` はアプリ全体で 1 個・
    /// `Arc` で寿命管理されており、FFmpeg の hw_device_ctx (AVBufferRef) より長生きする
    /// ので、このポインタは AVBufferRef の寿命中ずっと有効。
    pub fn d3d_lock_ptr(&self) -> *mut SRWLOCK {
        self.d3d_lock.get()
    }

    /// `blit_nv12_to_rgba` の context 操作区間で握る RAII guard を返す。FFmpeg の
    /// D3D11VA lock/unlock callback と同じ SRWLOCK なので、FFmpeg decode と blit の
    /// `ID3D11DeviceContext` / `ID3D11VideoContext` 操作が直列化される。
    fn lock_d3d_context(&self) -> D3dContextGuard<'_> {
        // SAFETY: SRWLOCK は共有ポインタ越しの並行 Acquire/Release を前提に設計された
        // OS プリミティブ。`UnsafeCell` で内部可変を表現し、`GpuVideoDevice` の
        // `unsafe impl Sync` がこの共有を保証する。
        unsafe {
            AcquireSRWLockExclusive(self.d3d_lock.get());
        }
        D3dContextGuard {
            lock: self.d3d_lock.get(),
            _marker: std::marker::PhantomData,
        }
    }

    /// 入力 NV12 / P010 テクスチャを RGBA 共有テクスチャに blit する。
    /// 出力テクスチャは新規作成 (リング管理は呼び出し側)。
    ///
    /// SAFETY: `input_texture` / `subresource` が valid であること。FFmpeg の AVFrame
    /// から渡す前提。
    pub unsafe fn blit_nv12_to_rgba(
        &self,
        input_texture: &ID3D11Texture2D,
        subresource: u32,
        active_w: u32,
        active_h: u32,
        out_w: u32,
        out_h: u32,
        ten_bit: bool,
        color_hint: VideoColorHint,
        input_fps_num: u32,
        input_fps_den: u32,
        pts_secs: f64,
    ) -> Result<BlitOutput, GpuVideoError> {
        // 1. 入力テクスチャの属性を読む。
        //    FFmpeg HW frames context の確保サイズは 16 ピクセルアライン (= coded_width/height)
        //    で、AVFrame の `width()/height()` (active_w/active_h) より大きいことがある
        //    (例: 1280x720 動画 → 1280x736 テクスチャ)。video processor 用 content_desc
        //    にはテクスチャ寸法を渡し、実 active 領域は SourceRect で指定する。
        let in_desc = unsafe {
            let mut d = D3D11_TEXTURE2D_DESC::default();
            input_texture.GetDesc(&mut d);
            d
        };
        let in_w = in_desc.Width;
        let in_h = in_desc.Height;
        let in_format = in_desc.Format;
        // mIV does not expose an HDR display path. Normalize even P010/P016
        // sources to SDR BGRA8 here so native DComp and legacy wgpu consume one
        // display texture format.
        let out_format = DXGI_FORMAT_B8G8R8A8_UNORM;

        // 2. processor / enumerator を確保 (キャッシュ)
        self.ensure_processor(
            in_w,
            in_h,
            in_format,
            out_w,
            out_h,
            out_format,
            input_fps_num,
            input_fps_den,
        )?;
        let cache = self.processor_cache.lock().unwrap();
        let state = cache.as_ref().expect("ensured above");

        // 3. 出力先を 2 段構成にする (vsr_probe で driver 仕様確定):
        //    - 中間 RT テクスチャ (NT/KM なし) → VideoProcessorBlt の宛先
        //    - 共有 RGBA テクスチャ (NT|KEYEDMUTEX) → wgpu D3D12 OpenSharedHandle 用
        //    NVIDIA driver:
        //      * NT shared 単独 (= NTHANDLE のみ) は CreateTexture2D で E_INVALIDARG
        //        (vsr_probe の flags-probe で全 size/format 一様に確認)
        //      * 同 driver は KEYEDMUTEX 付き NT shared を VideoProcessorBlt 出力として
        //        E_INVALIDARG で拒否する (= 過去観察)
        //    ⇒ VPP 出力は NT/KM どちらも持たない intermediate に出して、CopyResource で
        //       NT|KM 持ちの共有テクスチャに転送する。同期は keyed mutex には依存せず
        //       (D3D12 側で IDXGIKeyedMutex 取得不可)、ID3D11Fence ↔ ID3D12Fence の
        //       共有 fence で行う (`Signal`/`Wait`)。
        let intermediate = self.create_intermediate_rt(out_w, out_h, out_format)?;
        let (
            out_tex,
            shared_handle,
            close_shared_handle_on_drop,
            shared_output_in_use,
            shared_output_notify,
            shared_output_keyed_mutex,
            shared_output_released_to_reader,
            km_acquired,
            shared_texture_gen,
        ) = self.acquire_shared_output(out_w, out_h, out_format)?;

        // Codex P1 (2026-05-15): acquire 直後に guard を作り、以降の `?` 早期 return で
        // 必ず slot を返却するようにする (= ReleaseSync(0) + in_use=false + notify)。
        // 旧コードは CreateInputView/OutputView/Blt/Signal の失敗で slot を永久占有していた。
        let mut slot_guard = SharedOutputSlotGuard::new(
            shared_output_in_use.clone(),
            shared_output_notify.clone(),
            km_acquired.clone(),
            km_acquired.is_some(),
        );

        // shared_handle は CreateInputView/OutputView/Blt のいずれかが失敗して `?` で
        // 早期リターンされても close する必要がある (Codex P2)。BlitOutput を返す直前で
        // disarm() してリーク防止責任を呼び出し側 (D3d11Frame::Drop) に移譲する。
        // Shared output handles are owned by `shared_output_pool` and reused.

        // ここから先は `ID3D11VideoContext` / `ID3D11DeviceContext` を触る。FFmpeg の
        // D3D11VA decode (`avcodec_send_packet` 等) が同じ context を別スレッドから
        // 並行使用しうるため、`d3d_lock` SRWLOCK で直列化する (FFmpeg 側は
        // `AVD3D11VADeviceContext.lock/unlock` callback で同じ SRWLOCK を握る)。
        // `acquire_shared_output` (最大 500ms wait しうる) は **この lock の外**で
        // 済ませてあるので、FFmpeg decode を不必要に長くブロックしない。guard は
        // 関数末尾 (BlitOutput 返却まで) で drop される。
        let _d3d_guard = self.lock_d3d_context();

        // 4. 入力 view を作成
        let mut in_view: Option<ID3D11VideoProcessorInputView> = None;
        let in_view_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: windows::Win32::Graphics::Direct3D11::D3D11_TEX2D_VPIV {
                    MipSlice: 0,
                    ArraySlice: subresource,
                },
            },
        };
        unsafe {
            self.video_device
                .CreateVideoProcessorInputView(
                    input_texture,
                    &state.enumerator,
                    &in_view_desc,
                    Some(&mut in_view),
                )
                .map_err(|e| GpuVideoError::from_blt_hresult("CreateInputView", e))?;
        }
        let in_view = in_view.ok_or_else(|| GpuVideoError::Blt("InputView null".into()))?;

        // 5. 出力 view を中間テクスチャに対して作る (NT shared 付きでない普通の RT)。
        let mut out_view: Option<ID3D11VideoProcessorOutputView> = None;
        let out_view_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: windows::Win32::Graphics::Direct3D11::D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        };
        unsafe {
            self.video_device
                .CreateVideoProcessorOutputView(
                    &intermediate,
                    &state.enumerator,
                    &out_view_desc,
                    Some(&mut out_view),
                )
                .map_err(|e| GpuVideoError::from_blt_hresult("CreateOutputView", e))?;
        }
        let out_view = out_view.ok_or_else(|| GpuVideoError::Blt("OutputView null".into()))?;

        // 6. SourceRect / DestRect を明示。FFmpeg の coded_width/height アライン由来で
        //    テクスチャは active 領域より大きい場合があるため、active 領域を SourceRect
        //    としてクロップ + 出力フル領域に拡大する。これを設定しないと processor は
        //    full texture → full output で blit しようとして黒帯ピクセルもアップスケール
        //    対象になり、また driver によっては寸法不一致で E_INVALIDARG を返す。
        unsafe {
            use windows::Win32::Foundation::RECT;
            let src_rect = RECT {
                left: 0,
                top: 0,
                right: active_w as i32,
                bottom: active_h as i32,
            };
            let dst_rect = RECT {
                left: 0,
                top: 0,
                right: out_w as i32,
                bottom: out_h as i32,
            };
            self.video_context.VideoProcessorSetStreamSourceRect(
                &state.processor,
                0,
                true,
                Some(&src_rect),
            );
            self.video_context.VideoProcessorSetStreamDestRect(
                &state.processor,
                0,
                true,
                Some(&dst_rect),
            );
        }

        // 100 frame ごとに 1 行診断ログ (= ~1.6 秒に 1 行 @ 60fps)。
        {
            use std::sync::atomic::{AtomicU64, Ordering};
            static FRAME_COUNT: AtomicU64 = AtomicU64::new(0);
            let n = FRAME_COUNT.fetch_add(1, Ordering::Relaxed);
            if n == 0 || n % 100 == 0 {
                crate::logger::log(format!(
                    "VPP blit #{n} pts={pts_secs:.2}s in={active_w}x{active_h} \
                     ({:?} {}) out={out_w}x{out_h} ({:?}) fps={}/{} hint={:?}",
                    in_format,
                    if ten_bit { "10b-src" } else { "8b-src" },
                    out_format,
                    input_fps_num,
                    input_fps_den,
                    color_hint,
                ));
            }
        }

        // 色空間ヒントを毎 Blt で reassert する。
        //   - SDR: BT.709 studio range G22 (NV12 と SDR 10-bit P010 を区別しない)
        //   - HDR PQ: BT.2020 studio range G2084
        //   - HDR HLG: BT.2020 studio range GHLG
        // 出力は固定 RGB sRGB (BT.709 G22 full range)。HDR 入力は VPP がトーンマップする。
        let in_color_space: DXGI_COLOR_SPACE_TYPE = match color_hint {
            VideoColorHint::Sdr => DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
            VideoColorHint::HdrPq => DXGI_COLOR_SPACE_YCBCR_STUDIO_G2084_LEFT_P2020,
            VideoColorHint::HdrHlg => DXGI_COLOR_SPACE_YCBCR_STUDIO_GHLG_TOPLEFT_P2020,
        };
        let out_color_space = DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709;
        unsafe {
            self.video_context1.VideoProcessorSetStreamColorSpace1(
                &state.processor,
                0,
                in_color_space,
            );
            self.video_context1
                .VideoProcessorSetOutputColorSpace1(&state.processor, out_color_space);
        }

        // ストリームを構成して blit
        let stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            OutputIndex: 0,
            InputFrameOrField: 0,
            PastFrames: 0,
            FutureFrames: 0,
            ppPastSurfaces: ptr::null_mut(),
            pInputSurface: std::mem::ManuallyDrop::new(Some(in_view.clone())),
            ppFutureSurfaces: ptr::null_mut(),
            ppPastSurfacesRight: ptr::null_mut(),
            pInputSurfaceRight: std::mem::ManuallyDrop::new(None),
            ppFutureSurfacesRight: ptr::null_mut(),
        };
        unsafe {
            self.video_context
                .VideoProcessorBlt(&state.processor, &out_view, 0, &[stream])
                .map_err(|e| {
                    crate::logger::log(format!(
                        "Blt FAIL: in {}x{} {:?} array={} sub={} -> out {}x{} {:?}: err={e:?}",
                        in_w,
                        in_h,
                        in_format,
                        in_desc.ArraySize,
                        subresource,
                        out_w,
                        out_h,
                        out_format,
                    ));
                    GpuVideoError::from_blt_hresult("Blt", e)
                })?;
            // Blt 完了後、中間テクスチャ (NT/KM なし) の内容を NT|KM 共有テクスチャに
            // コピーする。同 D3D11 device 内 GPU copy なので latency は ~0.1ms オーダ。
            //
            // **重要**: KEYEDMUTEX 付きテクスチャは IDXGIKeyedMutex の AcquireSync/
            // ReleaseSync を D3D11 側で呼ばないと、D3D12 OpenSharedHandle 経由で
            // 読み取った時に内容ゼロ (= 黒画面) になる。out_tex は毎フレーム新規作成で
            // 初期 key=0 から始まるので、毎回 AcquireSync(0) → write → ReleaseSync(1)
            // を呼べば独立に完結する (key 状態を frame 間で持ち越す必要なし)。
            // 同期自体はこの mutex には依存せず ID3D11Fence ↔ ID3D12Fence で行う。
            // INFINITE timeout は危険 (driver state 異常で永久ブロック)。
            // 100ms timeout で fail-fast、タイムアウト時はそのフレームを諦めて Err を返す。
            // 通常は fresh out_tex (= released-with-key-0 状態) なので即取れる。
            self.context.CopyResource(&out_tex, &intermediate);
            // KeyedMutex を release (key=1) — D3D12 OpenSharedHandle 側はこれ以降の状態
            // (= "key=1 で release 済み") を読み取れる。AcquireSync 自体はしない (= D3D12 の
            // ID3D12Resource は IDXGIKeyedMutex を取得できないため)。実 sync は fence 任せ。
            // ReleaseSync 失敗時は frame を発行しない (Codex P3): release されないと D3D12
            // 側の OpenSharedHandle が空テクスチャしか読めなくなる。
            if let Some(km) = km_acquired {
                if let Err(e) = km.ReleaseSync(1) {
                    crate::logger::log(format!("KeyedMutex ReleaseSync(1) failed: {e:?}"));
                    // guard armed のまま return → Drop で ReleaseSync(0) + in_use=false。
                    return Err(GpuVideoError::Blt(format!("KeyedMutex ReleaseSync: {e:?}")));
                }
                // write key を reader 側に渡した → 以降 guard 経路で ReleaseSync(0) を
                // 呼んではいけない (Codex P1)。
                slot_guard.mark_released_to_reader();
                if let Some(released) = &shared_output_released_to_reader {
                    released.store(true, Ordering::Release);
                }
            }
            self.context.Flush();
        }
        // 共有 fence を 1 進めて Signal。D3D12 側が `Wait(fence, fence_value)` で
        // GPU レベルの待ち合わせをする。Flush() は queue にコマンドを投入するだけで
        // GPU 完了は待たないが、Signal はその queue に入れた直後に置かれるので、
        // GPU 上では「CopyResource 完了 → Signal」の順序が保証される。
        let fence_value = self
            .next_fence_value
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
            + 1;
        unsafe {
            self.context4
                .Signal(&self.fence, fence_value)
                .map_err(|e| GpuVideoError::Blt(format!("Fence Signal({fence_value}): {e:?}")))?;
        }

        // 全成功: HANDLE 所有権を BlitOutput → D3d11Frame に移譲する。以降 guard が drop
        // しても close は走らない (= D3d11Frame::Drop の責任になる)。
        // 同様に slot_guard も disarm: in_use=false / ReleaseSync は D3d11Frame::Drop と
        // reader 側に責任移譲する (Codex P1 2026-05-15)。
        slot_guard.disarm();
        Ok(BlitOutput {
            output_texture: out_tex,
            shared_handle,
            close_shared_handle_on_drop,
            shared_output_in_use,
            shared_output_notify,
            shared_output_keyed_mutex,
            shared_output_released_to_reader,
            fence_value,
            shared_texture_gen,
        })
    }

    fn ensure_processor(
        &self,
        in_w: u32,
        in_h: u32,
        in_format: DXGI_FORMAT,
        out_w: u32,
        out_h: u32,
        out_format: DXGI_FORMAT,
        input_fps_num: u32,
        input_fps_den: u32,
    ) -> Result<(), GpuVideoError> {
        // ContentDesc の InputFrameRate に実 fps を渡す。ドライバは内部スケジューラで
        // フレームレートを参照している可能性があり、嘘値 (60/1 ハードコード) を渡すと
        // mid-stream で「規定外コンテンツ」と判定されることがある。
        // fps が分からないケース (= 0) は安全側で 60/1 にフォールバック。
        let (fps_num, fps_den) = if input_fps_num == 0 || input_fps_den == 0 {
            (60u32, 1u32)
        } else {
            (input_fps_num, input_fps_den)
        };

        let cur = self.processor_cache.lock().unwrap();
        if let Some(s) = cur.as_ref() {
            if s.in_w == in_w
                && s.in_h == in_h
                && s.in_format == in_format
                && s.out_w == out_w
                && s.out_h == out_h
                && s.out_format == out_format
                && s.fps_num == fps_num
                && s.fps_den == fps_den
            {
                return Ok(());
            }
        }
        drop(cur);
        let content_desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: DXGI_RATIONAL {
                Numerator: fps_num,
                Denominator: fps_den,
            },
            InputWidth: in_w,
            InputHeight: in_h,
            OutputFrameRate: DXGI_RATIONAL {
                Numerator: fps_num,
                Denominator: fps_den,
            },
            OutputWidth: out_w,
            OutputHeight: out_h,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        };
        let enumerator = unsafe {
            self.video_device
                .CreateVideoProcessorEnumerator(&content_desc)
                .map_err(|e| {
                    // E_OUTOFMEMORY 等は ResourcePressure に振り分け (Codex P1 2026-05-15)。
                    if GpuVideoError::hresult_is_resource_pressure(&e) {
                        GpuVideoError::ResourcePressure(format!(
                            "CreateVideoProcessorEnumerator: {e:?}"
                        ))
                    } else {
                        GpuVideoError::EnumeratorCreate(format!("{e:?}"))
                    }
                })?
        };
        let processor = unsafe {
            self.video_device
                .CreateVideoProcessor(&enumerator, 0)
                .map_err(|e| {
                    if GpuVideoError::hresult_is_resource_pressure(&e) {
                        GpuVideoError::ResourcePressure(format!("CreateVideoProcessor: {e:?}"))
                    } else {
                        GpuVideoError::ProcessorCreate(format!("{e:?}"))
                    }
                })?
        };
        *self.processor_cache.lock().unwrap() = Some(ProcessorState {
            enumerator,
            processor,
            in_w,
            in_h,
            in_format,
            out_w,
            out_h,
            out_format,
            fps_num,
            fps_den,
        });
        Ok(())
    }

    /// 中間テクスチャ (= NT shared フラグなし、純粋に D3D11 内部の RT)。
    /// `blit_nv12_to_rgba` の 2 段構成 (VPP 出力 → CopyResource → 共有テクスチャ) の
    /// 1 段目で使う。NVIDIA driver は VPP 出力に NT/KM 付き shared テクスチャを許さない。
    fn create_intermediate_rt(
        &self,
        w: u32,
        h: u32,
        format: DXGI_FORMAT,
    ) -> Result<ID3D11Texture2D, GpuVideoError> {
        let bind_flags = (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: bind_flags,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut tex: Option<ID3D11Texture2D> = None;
        unsafe {
            self.device
                .CreateTexture2D(&desc, None, Some(&mut tex))
                .map_err(|e| {
                    // E_OUTOFMEMORY (0x8007000E) など pressure 系は ResourcePressure に
                    // 振り分け、decoder 側で CPU fallback させない (Codex P1 2026-05-15)。
                    if GpuVideoError::hresult_is_resource_pressure(&e) {
                        GpuVideoError::ResourcePressure(format!(
                            "CreateTexture2D intermediate {w}x{h} format={format:?}: {e:?}"
                        ))
                    } else {
                        GpuVideoError::TextureCreate(format!(
                            "intermediate {w}x{h} format={format:?} bind=0x{bind_flags:X}: {e:?}"
                        ))
                    }
                })?;
        }
        tex.ok_or_else(|| GpuVideoError::TextureCreate("intermediate null".into()))
    }

    fn recover_shared_output_keyed_mutex(
        mutex: &IDXGIKeyedMutex,
        released_to_reader: &AtomicBool,
        shared_handle: HANDLE,
        expected_released_to_reader: bool,
    ) -> bool {
        let recover_t0 = std::time::Instant::now();
        let Ok(()) = (unsafe { mutex.AcquireSync(1, 0) }) else {
            return false;
        };
        unsafe {
            let _ = mutex.ReleaseSync(0);
        }
        released_to_reader.store(false, Ordering::Release);
        crate::perf::event(
            "video",
            "shared_output_keyed_mutex_recovered",
            None,
            0,
            &[
                (
                    "shared_handle",
                    serde_json::Value::from(shared_handle.0 as usize as u64),
                ),
                (
                    "recover_ms",
                    serde_json::Value::from(recover_t0.elapsed().as_secs_f64() * 1000.0),
                ),
                (
                    "expected_released_to_reader",
                    serde_json::Value::from(expected_released_to_reader),
                ),
            ],
        );
        true
    }

    fn acquire_shared_output(
        &self,
        w: u32,
        h: u32,
        format: DXGI_FORMAT,
    ) -> Result<
        (
            ID3D11Texture2D,
            HANDLE,
            bool,
            Option<Arc<AtomicBool>>,
            Option<Arc<Condvar>>,
            Option<IDXGIKeyedMutex>,
            Option<Arc<AtomicBool>>,
            Option<IDXGIKeyedMutex>,
            // SharedOutputSlot::texture_gen — presenter のテクスチャキャッシュ identity 用。
            u64,
        ),
        GpuVideoError,
    > {
        // プロセス内ユニークな共有出力テクスチャ世代 ID。0 は予約 (未設定扱い)、
        // slot 生成ごとに 1, 2, 3, ... と進む。HANDLE 値は OS が再利用しうるので
        // 別軸の identity が必要 (fence_gen と同じ理由)。
        static NEXT_SHARED_TEXTURE_GEN: std::sync::atomic::AtomicU64 =
            std::sync::atomic::AtomicU64::new(1);
        let wait_started = std::time::Instant::now();
        loop {
            let mut pool = self
                .shared_output_pool
                .lock()
                .map_err(|_| GpuVideoError::Blt("shared output pool poisoned".into()))?;

            for slot in pool
                .iter()
                .filter(|slot| slot.width == w && slot.height == h && slot.format == format)
            {
                let Ok(false) =
                    slot.in_use
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                else {
                    continue;
                };
                let released_to_reader = Arc::clone(&slot.released_to_reader);
                let slot_gen = slot.texture_gen;
                let Ok(km) = slot.tex.cast::<IDXGIKeyedMutex>() else {
                    return Ok((
                        slot.tex.clone(),
                        slot.shared_handle,
                        false,
                        Some(Arc::clone(&slot.in_use)),
                        Some(Arc::clone(&self.shared_output_pool_cv)),
                        None,
                        Some(released_to_reader),
                        None,
                        slot_gen,
                    ));
                };
                let expected_released_to_reader = released_to_reader.load(Ordering::Acquire);
                let mut recovered_key_one = false;
                let acquired_for_write = if expected_released_to_reader {
                    recovered_key_one = Self::recover_shared_output_keyed_mutex(
                        &km,
                        &released_to_reader,
                        slot.shared_handle,
                        expected_released_to_reader,
                    );
                    recovered_key_one && (unsafe { km.AcquireSync(0, 0) }).is_ok()
                } else {
                    (unsafe { km.AcquireSync(0, 0) }).is_ok() || {
                        recovered_key_one = Self::recover_shared_output_keyed_mutex(
                            &km,
                            &released_to_reader,
                            slot.shared_handle,
                            expected_released_to_reader,
                        );
                        recovered_key_one && (unsafe { km.AcquireSync(0, 0) }).is_ok()
                    }
                };
                if acquired_for_write {
                    released_to_reader.store(false, Ordering::Release);
                    return Ok((
                        slot.tex.clone(),
                        slot.shared_handle,
                        false,
                        Some(Arc::clone(&slot.in_use)),
                        Some(Arc::clone(&self.shared_output_pool_cv)),
                        Some(km.clone()),
                        Some(released_to_reader),
                        Some(km),
                        slot_gen,
                    ));
                }
                crate::perf::event(
                    "video",
                    "shared_output_acquire_timeout",
                    None,
                    0,
                    &[
                        (
                            "shared_handle",
                            serde_json::Value::from(slot.shared_handle.0 as usize as u64),
                        ),
                        (
                            "released_to_reader",
                            serde_json::Value::from(expected_released_to_reader),
                        ),
                        (
                            "recovered_key_one",
                            serde_json::Value::from(recovered_key_one),
                        ),
                    ],
                );
                slot.in_use.store(false, Ordering::Release);
                self.shared_output_pool_cv.notify_one();
                continue;
            }

            if pool.len() >= OUTPUT_RING_SIZE {
                if let Some(pos) = pool.iter().position(|slot| {
                    (slot.width != w || slot.height != h || slot.format != format)
                        && !slot.in_use.load(Ordering::Acquire)
                }) {
                    let evicted = pool.remove(pos);
                    crate::perf::event(
                        "video",
                        "shared_output_pool_evict",
                        None,
                        0,
                        &[
                            ("width", serde_json::Value::from(evicted.width as i64)),
                            ("height", serde_json::Value::from(evicted.height as i64)),
                            ("requested_width", serde_json::Value::from(w as i64)),
                            ("requested_height", serde_json::Value::from(h as i64)),
                            ("pool_len", serde_json::Value::from(pool.len() as i64)),
                            (
                                "shared_handle",
                                serde_json::Value::from(evicted.shared_handle.0 as usize as u64),
                            ),
                        ],
                    );
                    drop(evicted);
                }
            }

            if pool.len() < OUTPUT_RING_SIZE {
                let (tex, shared_handle) = self.create_shared_output(w, h, format)?;
                let slot_gen =
                    NEXT_SHARED_TEXTURE_GEN.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                let in_use = Arc::new(AtomicBool::new(true));
                let released_to_reader = Arc::new(AtomicBool::new(false));
                let km_acquired = match tex.cast::<IDXGIKeyedMutex>() {
                    Ok(km) => match unsafe { km.AcquireSync(0, 100) } {
                        Ok(()) => Some(km),
                        Err(e) => {
                            in_use.store(false, Ordering::Release);
                            unsafe {
                                let _ = windows::Win32::Foundation::CloseHandle(shared_handle);
                            }
                            return Err(GpuVideoError::Blt(format!(
                                "KeyedMutex AcquireSync: {e:?}"
                            )));
                        }
                    },
                    Err(e) => {
                        crate::logger::log(format!("cast IDXGIKeyedMutex failed: {e:?}"));
                        None
                    }
                };
                pool.push(SharedOutputSlot {
                    tex: tex.clone(),
                    shared_handle,
                    width: w,
                    height: h,
                    format,
                    in_use: Arc::clone(&in_use),
                    released_to_reader: Arc::clone(&released_to_reader),
                    texture_gen: slot_gen,
                });
                crate::perf::event(
                    "video",
                    "shared_output_pool_grow",
                    None,
                    0,
                    &[
                        ("width", serde_json::Value::from(w as i64)),
                        ("height", serde_json::Value::from(h as i64)),
                        ("pool_len", serde_json::Value::from(pool.len() as i64)),
                        (
                            "shared_handle",
                            serde_json::Value::from(shared_handle.0 as usize as u64),
                        ),
                    ],
                );
                return Ok((
                    tex,
                    shared_handle,
                    false,
                    Some(in_use),
                    Some(Arc::clone(&self.shared_output_pool_cv)),
                    km_acquired.clone(),
                    Some(released_to_reader),
                    km_acquired,
                    slot_gen,
                ));
            }

            drop(pool);
            if wait_started.elapsed() >= std::time::Duration::from_millis(500) {
                // 2026-05-15: `Blt` → `ResourcePressure` に変更。decoder 側は
                // この variant を受けたら CPU readback fallback せず frame drop に
                // 落とす (CPU fallback は内部 swscale + GPU upload で adapter memory
                // をさらに食い OOM 連鎖を加速させる、Codex 助言)。
                return Err(GpuVideoError::ResourcePressure(
                    "shared output pool exhausted waiting for free slot".into(),
                ));
            }
            let guard = self
                .shared_output_pool_wait
                .lock()
                .map_err(|_| GpuVideoError::Blt("shared output pool wait poisoned".into()))?;
            let _ = self
                .shared_output_pool_cv
                .wait_timeout(guard, std::time::Duration::from_millis(4));
        }
    }

    /// 2 段アーキ用 shared output (CopyResource の宛先)。
    /// vsr_probe の flags-probe で観測した driver 仕様:
    ///   - `D3D11_RESOURCE_MISC_SHARED_NTHANDLE` 単独は CreateTexture2D で E_INVALIDARG
    ///   - `NTHANDLE | KEYEDMUTEX` の組合せは全 size/format で OK
    /// よって NT 共有を作るには KEYEDMUTEX flag が必須 (同 driver の隠れ仕様)。
    /// ただし keyed mutex 同期自体は使わない (D3D12 側で IDXGIKeyedMutex を取得できないため、
    /// 同期は ID3D11Fence ↔ ID3D12Fence の共有 fence で実装、`AcquireSync/ReleaseSync` も
    /// 呼ばない)。flag は CreateTexture2D 通すための形式的なもの。
    fn create_shared_output(
        &self,
        w: u32,
        h: u32,
        format: DXGI_FORMAT,
    ) -> Result<(ID3D11Texture2D, HANDLE), GpuVideoError> {
        let bind_flags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
        let misc_flags = (D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0
            | D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0) as u32;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: bind_flags,
            CPUAccessFlags: 0,
            // KEYEDMUTEX は付けない: 真に正しく実装するなら AcquireSync/ReleaseSync の
            // ペアを D3D11 側 (write) と D3D12 側 (read) 両方に入れる必要があるが、
            // ID3D12Resource からは IDXGIKeyedMutex を取れないため D3D12 側で実装が困難。
            // 代わりに ID3D11Fence ↔ ID3D12Fence の共有 fence で同期している (`Signal`/`Wait`)。
            MiscFlags: misc_flags,
        };
        let mut tex: Option<ID3D11Texture2D> = None;
        unsafe {
            self.device
                .CreateTexture2D(&desc, None, Some(&mut tex))
                .map_err(|e| {
                    // E_OUTOFMEMORY (0x8007000E) など pressure 系は ResourcePressure に振り分け
                    // (Codex 助言 2026-05-15)。CreateTexture2D は adapter memory 枯渇で
                    // E_OUTOFMEMORY を返すので、CPU fallback に逃がさず frame drop させる。
                    if GpuVideoError::hresult_is_resource_pressure(&e) {
                        GpuVideoError::ResourcePressure(format!(
                            "CreateTexture2D shared_output {w}x{h} format={format:?}: {e:?}"
                        ))
                    } else {
                        GpuVideoError::TextureCreate(format!(
                            "shared_output {w}x{h} format={format:?} bind=0x{bind_flags:X} \
                             misc=0x{misc_flags:X}: {e:?}"
                        ))
                    }
                })?;
        }
        let tex = tex.ok_or_else(|| GpuVideoError::TextureCreate("null texture".into()))?;
        let dxgi: IDXGIResource1 = tex
            .cast()
            .map_err(|e| GpuVideoError::SharedHandle(format!("cast IDXGIResource1: {e:?}")))?;
        let handle = unsafe {
            dxgi.CreateSharedHandle(None, windows::Win32::Foundation::GENERIC_ALL.0, None)
                .map_err(|e| GpuVideoError::SharedHandle(format!("{e:?}")))?
        };
        Ok((tex, handle))
    }
}

impl Drop for GpuVideoDevice {
    fn drop(&mut self) {
        // fence の NT shared handle は CreateSharedHandle で取得しており、本構造体が
        // 唯一の所有者。device drop タイミングで close する。D3D12 側で OpenSharedHandle
        // 経由で得た ID3D12Fence COM オブジェクトは内部参照を持っているので、ここで
        // close しても D3D12 側の fence は生存する (NT カーネルオブジェクトは refcount)。
        if !self.fence_shared_handle.is_invalid() {
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(self.fence_shared_handle);
            }
        }
    }
}

// ID3D11Device / ID3D11DeviceContext は COM オブジェクトで、`windows` crate の
// `Interface` 実装によって `Send + Sync` (内部 refcount は thread-safe な atomic)。
// ただし ID3D11DeviceContext の **同時呼び出し** は未定義動作 → 利用側で Mutex 化すること。
unsafe impl Send for GpuVideoDevice {}
unsafe impl Sync for GpuVideoDevice {}
