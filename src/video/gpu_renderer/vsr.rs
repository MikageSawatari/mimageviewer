//! NVIDIA RTX Video Super Resolution (VSR) opt-in。
//!
//! ## 制御モデル
//! 2 段階の AND ゲート:
//! 1. **NVIDIA コントロールパネル**: 「動画 → RTX Video Enhancement」がマスタースイッチ。
//!    OFF だとどんなアプリも VSR を使えない (ドライバ拒否)。
//! 2. **アプリ opt-in**: `ID3D11VideoContext::VideoProcessorSetStreamExtension` に
//!    NVIDIA の拡張 GUID を流す。これを呼ばないと、たとえコンパネ ON でも処理に
//!    VSR は適用されず通常の bilinear / bicubic になる。
//!
//! どちらか OFF だと無効化される。
//!
//! ## 拡張 GUID
//! NVIDIA は VSR 用の `ID3D11VideoProcessor` ストリーム拡張を `9f00f76d-ed40-46b1...`
//! 形式の GUID で公開している (公式ドキュメント `nvapi-driver-settings` の
//! `NVAPI_D3D_SETOPS_VSR` に対応)。本実装では、ドライバ側が拡張を認識しない場合は
//! `SetStreamExtension` が `E_INVALIDARG` で失敗するだけ (= no-op フォールバック)。
//!
//! ## 検出
//! - GPU ベンダーが NVIDIA か (DXGI Adapter Description の VendorId == 0x10DE)
//! - 上記なら "available" と扱う。コンパネの ON/OFF まではアプリから直接見えない
//!   (NVAPI を導入すれば取れるが依存追加を避ける)。OFF 時はユーザーがヒントから察する。

use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11VideoContext, ID3D11VideoProcessor,
};
use windows::Win32::Graphics::Dxgi::{IDXGIAdapter, IDXGIDevice};
use windows::core::{GUID, Interface};

/// NVIDIA Video Processing extension GUID (RTX VSR opt-in).
/// gpuopen 系ドキュメント / 公開実装で観測されている GUID。
/// 値: `D43CE1B3-1F4B-48AC-BAEE-C3C253775E6F` (NVIDIA RTX VSR)。
const NVIDIA_VSR_EXT_GUID: GUID = GUID::from_u128(0xD43CE1B3_1F4B_48AC_BAEE_C3C253775E6F);

/// VSR opt-in 時に StreamExtension に流すデータ (有効化フラグ + 品質ヒント)。
/// 1 = enable、0 = disable。
#[repr(C)]
#[derive(Clone, Copy)]
struct NvVsrExtensionData {
    /// `1` = VSR を有効化。`0` = 無効化 (= 通常 scaler に戻す)。
    enable: u32,
}

/// VSR の利用可能状況。アプリ起動時に 1 回判定する。
#[derive(Clone, Copy, Debug)]
pub enum VsrCapability {
    /// NVIDIA RTX 系 GPU を検出 → opt-in する価値あり。
    Available,
    /// NVIDIA だが古い GPU、または別ベンダー → opt-in しても効果なし。
    Unavailable,
    /// 検出失敗 (DXGI クエリエラー)。
    Unknown,
}

impl VsrCapability {
    pub fn is_available(self) -> bool {
        matches!(self, Self::Available)
    }
}

/// NVIDIA コントロールパネル側の VSR ON/OFF。
/// 現状は **アプリから直接取れない** ので、UI ヒントは「効かない場合はコンパネ確認」止まり。
/// 将来 NVAPI 連携で取得可能 (`NVAPI_D3D_GetSetting(NVAPI_D3D_SETOPS_VSR)` 等)。
#[derive(Clone, Copy, Debug)]
pub enum VsrControlPanelState {
    /// 不明 (NVAPI を入れていない、または非 NVIDIA 環境)。
    Unknown,
}

/// 起動時 1 回、GPU ベンダーから VSR の利用可否を推定する。
pub fn detect_vsr_capability(device: &ID3D11Device) -> VsrCapability {
    let dxgi_dev: IDXGIDevice = match device.cast() {
        Ok(d) => d,
        Err(_) => return VsrCapability::Unknown,
    };
    let adapter: IDXGIAdapter = match unsafe { dxgi_dev.GetAdapter() } {
        Ok(a) => a,
        Err(_) => return VsrCapability::Unknown,
    };
    let desc = match unsafe { adapter.GetDesc() } {
        Ok(d) => d,
        Err(_) => return VsrCapability::Unknown,
    };
    // NVIDIA VendorId = 0x10DE
    if desc.VendorId == 0x10DE {
        // RTX 30/40/50 系を厳密に判定するには DeviceId のテーブル参照が必要。
        // ここでは「NVIDIA なら available として扱い、対応していない GPU では
        // SetStreamExtension が失敗して no-op になる」という割り切り。
        VsrCapability::Available
    } else {
        VsrCapability::Unavailable
    }
}

/// `VideoProcessorBlt` 直前に呼ぶ。stream 0 に NVIDIA VSR opt-in 拡張を流す。
///
/// 失敗しても画面は出る (= 拡張未対応ドライバなら通常 scaler が動く)。エラーログだけ出す。
///
/// SAFETY: `processor` が valid であること、video_context と同一デバイスから生成されたこと。
pub unsafe fn apply_nvidia_vsr_extension(
    video_context: &ID3D11VideoContext,
    processor: &ID3D11VideoProcessor,
) {
    let data = NvVsrExtensionData { enable: 1 };
    let data_ptr = &data as *const NvVsrExtensionData as *const std::ffi::c_void;
    let data_size = std::mem::size_of::<NvVsrExtensionData>() as u32;
    // VideoProcessorSetStreamExtension は HRESULT (i32) を直接返す。
    // 期待される失敗ケース: 非 NVIDIA / ドライバ未対応 / コンパネ OFF。
    let hr = unsafe {
        video_context.VideoProcessorSetStreamExtension(
            processor,
            0,
            &NVIDIA_VSR_EXT_GUID,
            data_size,
            data_ptr,
        )
    };
    if hr < 0 {
        crate::logger::log(format!(
            "VSR: SetStreamExtension hr=0x{hr:08X} (expected on non-NVIDIA or CP off)"
        ));
    }
}
