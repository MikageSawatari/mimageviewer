//! 360 度パノラマビュー機能の App 側ステート・検出・キャッシュキー解決。
//!
//! GPU 描画は [`panorama_wgpu`](crate::panorama_wgpu)、WGSL シェーダや
//! `wgpu::Texture` 管理はそちらに分離。本ファイルは:
//!
//! - [`PanoramaState`] — yaw / pitch / fov_y / drag state を持つ UI ステート
//! - [`PanoramaTrigger`] — 検出結果 (Auto / Hint)
//! - [`PanoSourceResolution`] — `resolve_pano_source` の戻り値
//! - `make_pano_cache_key` / `crc16_of_str` — cache_key (u64 packed) の構築
//! - 各種 source_kind 定数
//!
//! 設計詳細は [docs/panorama-360-view-plan.md](../docs/panorama-360-view-plan.md)。

/// アスペクト比 2:1 判定の許容幅 (1.95 〜 2.05)。`source_dims` の生値で判定する
/// (rotation_db や clamp 後の値ではない)。§2.1 参照。
pub const ASPECT_LOW: f32 = 1.95;
pub const ASPECT_HIGH: f32 = 2.05;

/// 360 ビューの FOV 範囲 (ラジアン)。約 11° 〜 150°。§3.3 / §5.2。
pub const FOV_MIN: f32 = 0.2;
pub const FOV_MAX: f32 = 2.6;

/// 初期 FOV (約 69°)。§5.1。
pub const FOV_DEFAULT: f32 = 1.2;

/// pitch のクランプ範囲。極を直視させない (asin 数値誤差で天井 / 床テクセルが暴れる)。
pub const PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.001;

/// `cache_key` の source_kind ビット (§4.1.2):
/// - 0 = fs_cache のみ (補正なし、AI なし)
/// - 1 = adjustment_cache (raw + 単純色調補正)
/// - 2 = ai_upscale_cache (AI のみ)
/// - 3 = adjustment_cache (AI + 補正)
pub const SOURCE_KIND_FS: u16 = 0;
pub const SOURCE_KIND_ADJUST_RAW: u16 = 1;
pub const SOURCE_KIND_AI: u16 = 2;
pub const SOURCE_KIND_AI_ADJUST: u16 = 3;

/// 360 ビューのインタラクティブステート (フルスクリーン内のみ Some)。
/// ファイル切替 / フルスクリーン退出で `panorama_state = None`。
/// 360 でない画像へナビした場合は **保持しつつ非アクティブ化** (= 同セッション
/// 内で 360 画像に戻ったら yaw/pitch/fov を引き継ぐ)。
#[derive(Clone, Debug)]
pub struct PanoramaState {
    /// 経度 (radians)。[-π, π]。初期 0 (or GPano hint)。
    pub yaw: f32,
    /// 緯度 (radians)。`[-PITCH_LIMIT, PITCH_LIMIT]`。
    pub pitch: f32,
    /// 視野角 Y 方向 (radians)。`[FOV_MIN, FOV_MAX]`。
    pub fov_y: f32,
    /// マウス左ドラッグ中か。
    pub drag_active: bool,
    /// 直前のポインタ位置 (`drag_active=true` のとき有効)。
    pub last_pointer: Option<egui::Pos2>,
    /// 初期 yaw / pitch (リセット時 / 検出時の hint)。
    /// ユーザーがリセットボタンを押すと yaw/pitch/fov_y がこの値に戻る。
    pub initial_yaw: f32,
    pub initial_pitch: f32,
}

impl PanoramaState {
    /// GPano hint に基づくデフォルト値を作る。hint が無ければ 0 を使う。
    pub fn new(initial_yaw: f32, initial_pitch: f32) -> Self {
        Self {
            yaw: initial_yaw,
            pitch: initial_pitch.clamp(-PITCH_LIMIT, PITCH_LIMIT),
            fov_y: FOV_DEFAULT,
            drag_active: false,
            last_pointer: None,
            initial_yaw,
            initial_pitch: initial_pitch.clamp(-PITCH_LIMIT, PITCH_LIMIT),
        }
    }

    /// 初期視点にリセット (ダブルクリック / リセットボタン)。drag 状態は維持しない。
    pub fn reset(&mut self) {
        self.yaw = self.initial_yaw;
        self.pitch = self.initial_pitch;
        self.fov_y = FOV_DEFAULT;
        self.drag_active = false;
        self.last_pointer = None;
    }
}

/// 部分 FOV equirect 画像 (GPano `CroppedArea*` 宣言) の UV 変換パラメータ。
///
/// **背景**: DSLR + nodal panhead で撮った 360 写真などは、水平 360° は撮れているが
/// 天頂と地面まで撮りきれず、フル球面の一部しか画像に含まれないケースが多い。
/// GPano XMP は `FullPanoWidthPixels` / `FullPanoHeightPixels` でフル球面の寸法を
/// 宣言し、`CroppedAreaImageWidthPixels` / `CroppedAreaImageHeightPixels` /
/// `CroppedAreaLeftPixels` / `CroppedAreaTopPixels` で画像が球面上で占める位置を
/// 表す。これを無視して画像全体をフル equirect として球に貼ると、画像の上端が
/// 天頂に紐付けられて水平線がずれる。
///
/// **WGSL での使い方**: 視線ベクトルから経度緯度経由で計算した
/// `sphere_uv ∈ [0,1]² (フル球面座標)` を、画像テクスチャ座標
/// `texture_uv = (sphere_uv - offset) / scale` で変換してからサンプル。
/// scale < 1 のとき、texture_uv が [0,1] 範囲外になった領域は AddressMode の
/// `ClampToEdge` で端の色を引き伸ばす (= 上下の欠けた領域は空 / 地面の色で
/// 自然に埋まる)。
///
/// **identity (フル equirect)**: `(u_offset, v_offset, u_scale, v_scale) = (0, 0, 1, 1)`。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanoUvTransform {
    pub u_offset: f32,
    pub v_offset: f32,
    pub u_scale: f32,
    pub v_scale: f32,
}

impl PanoUvTransform {
    /// フル equirect (画像 = フル球面) の場合の identity 変換。
    pub const IDENTITY: Self = Self {
        u_offset: 0.0,
        v_offset: 0.0,
        u_scale: 1.0,
        v_scale: 1.0,
    };

    /// このトランスフォームが identity (= 全 [0,1] のフル equirect) か。
    /// 識別子としては panorama_state や UV 経路の選択に影響しないが、cache_key 解析
    /// やデバッグ表示で「これは部分 FOV か?」を即判定できる。
    pub fn is_identity(&self) -> bool {
        self.u_offset == 0.0 && self.v_offset == 0.0 && self.u_scale == 1.0 && self.v_scale == 1.0
    }

    /// GPano XMP の `CroppedArea*` + `FullPano*` 値から UV transform を計算する。
    /// すべての必須フィールドが揃っているとき `Some(_)` を返し、欠けるか不正値
    /// (= ゼロ除算 / 範囲外) の場合は `None` (= フル equirect 扱いに fallback)。
    ///
    /// 計算式:
    /// - `u_scale = cropped_w / full_w` (画像が水平方向で占める割合)
    /// - `v_scale = cropped_h / full_h` (同 垂直方向)
    /// - `u_offset = cropped_left / full_w` (画像の左端がフル球面の何 % 地点か)
    /// - `v_offset = cropped_top / full_h` (画像の上端がフル球面の何 % 地点か)
    pub fn from_gpano(info: &crate::xmp_reader::XmpPanoramaInfo) -> Option<Self> {
        let full_w = info.full_pano_width_pixels? as f32;
        let full_h = info.full_pano_height_pixels? as f32;
        let cropped_w = info.cropped_area_image_width_pixels? as f32;
        let cropped_h = info.cropped_area_image_height_pixels? as f32;
        // Left / Top は 0 が valid なので unwrap_or(0) で許容するか、宣言があれば使う。
        // GPano 仕様上、CroppedAreaImage* があるなら CroppedAreaLeft/Top も提供するのが
        // 通例だが、無いケースは中央寄せと解釈 (= left = (full - cropped) / 2)。
        let cropped_left = info
            .cropped_area_left_pixels
            .map(|v| v as f32)
            .unwrap_or_else(|| ((full_w - cropped_w) * 0.5).max(0.0));
        let cropped_top = info
            .cropped_area_top_pixels
            .map(|v| v as f32)
            .unwrap_or_else(|| ((full_h - cropped_h) * 0.5).max(0.0));
        // 不正値チェック
        if full_w <= 0.0 || full_h <= 0.0 || cropped_w <= 0.0 || cropped_h <= 0.0 {
            return None;
        }
        // 範囲外 (cropped が full をはみ出す) は防衛的に identity 化
        if cropped_left + cropped_w > full_w * 1.001 || cropped_top + cropped_h > full_h * 1.001 {
            return None;
        }
        let xform = Self {
            u_offset: cropped_left / full_w,
            v_offset: cropped_top / full_h,
            u_scale: cropped_w / full_w,
            v_scale: cropped_h / full_h,
        };
        // 全領域 (差が 0.5% 以下) なら identity 扱いにして無駄な UV 変換を避ける。
        // 浮動小数の比較は厳密一致ではなく許容幅で判定。
        let near_full = (xform.u_offset.abs() < 0.005)
            && (xform.v_offset.abs() < 0.005)
            && ((1.0 - xform.u_scale).abs() < 0.005)
            && ((1.0 - xform.v_scale).abs() < 0.005);
        if near_full {
            Some(Self::IDENTITY)
        } else {
            Some(xform)
        }
    }
}

/// `App::detect_panorama` の戻り値。
///
/// - `Auto`: GPano XMP が `UsePanoramaViewer=True` + `ProjectionType=equirectangular` を
///   宣言している (= ビューアアプリでの全画面表示が推奨されているシグナル)。
/// - `Hint`: 弱いシグナル (アスペクト 2:1 のみ、または GPano `ProjectionType` のみ)。
///
/// **どちらでも自動 ON はしない** (フィードバック反映で廃止、機能制限モードに
/// 強制的に入るのは違和感が大きいため)。代わりに:
/// - `App::open_fullscreen` 時に「V キーで 360°ビューワー」案内トーストを出す
/// - ホバーバーに 360 ボタンを表示 (Auto はツールチップで強調、Hint は控えめ)
/// - V キーまたはボタンクリックで明示的にトグル
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanoramaTrigger {
    Auto,
    Hint,
}

/// `App::resolve_pano_source` の戻り値。8K base のアップロード元 / cache_key /
/// settle 判定 (Phase 2a) をまとめて 1 関数で決める (§4.3)。
pub struct PanoSourceResolution {
    /// `App::metadata_cache_key(idx)` の戻り値。`pano_uploaded.source_key` と比較。
    pub source_key: String,
    /// `(idx_hash, source_kind, adjust_gen, ai_gen)` を u64 にパック (§4.1.2)。
    pub cache_key: u64,
    /// 360 ベーステクスチャのアップロード元。`color_image_to_rgba` で RGBA8 化する。
    pub pixels: std::sync::Arc<egui::ColorImage>,
    /// どのキャッシュ層から取ったか (SOURCE_KIND_*)。Phase 2a の settle policy 判定にも使う。
    pub source_kind: u16,
}

/// 64-bit packed cache key:
///
/// ```text
/// [63..48]: idx_hash16   (CRC16 of source_key)
/// [47..32]: source_kind  (0=fs_cache, 1=raw+adj, 2=ai, 3=ai+adj)
/// [31..16]: adjust_gen16 (App::adjustment_generation[source_key] の下位 16bit)
/// [15..0] : ai_gen16     (App::ai_upscale_generation[source_key] の下位 16bit)
/// ```
///
/// 16 bit gen は 65,536 回の更新で wrap するが、長時間セッションの実害は低い
/// (§4.1.2 末尾の wrap 議論を参照)。Phase 3 で bit 再配分を検討する余地あり。
pub fn make_pano_cache_key(idx_hash: u16, source_kind: u16, adjust_gen: u16, ai_gen: u16) -> u64 {
    ((idx_hash as u64) << 48)
        | ((source_kind as u64) << 32)
        | ((adjust_gen as u64) << 16)
        | (ai_gen as u64)
}

/// 文字列の CRC-16/CCITT-FALSE 値。`source_key` (metadata_cache_key) を 16 bit に畳む。
///
/// 厳密な衝突回避が目的ではない (cache_key の他 3 要素と合わせて stale 検出するため、
/// idx_hash の衝突は別 source_kind / gen で吸収される)。軽量実装で十分。
pub fn crc16_of_str(s: &str) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for byte in s.as_bytes() {
        crc ^= (*byte as u16) << 8;
        for _ in 0..8 {
            if (crc & 0x8000) != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_packs_and_extracts() {
        let key = make_pano_cache_key(0xABCD, 2, 0x1234, 0x5678);
        assert_eq!((key >> 48) & 0xFFFF, 0xABCD);
        assert_eq!((key >> 32) & 0xFFFF, 2);
        assert_eq!((key >> 16) & 0xFFFF, 0x1234);
        assert_eq!(key & 0xFFFF, 0x5678);
    }

    #[test]
    fn cache_key_differs_when_source_kind_changes() {
        let a = make_pano_cache_key(0xABCD, 0, 1, 0);
        let b = make_pano_cache_key(0xABCD, 1, 1, 0);
        assert_ne!(a, b);
    }

    #[test]
    fn cache_key_differs_when_gen_changes() {
        let a = make_pano_cache_key(0xABCD, 1, 1, 0);
        let b = make_pano_cache_key(0xABCD, 1, 2, 0);
        assert_ne!(a, b);
        let c = make_pano_cache_key(0xABCD, 1, 1, 1);
        assert_ne!(a, c);
        assert_ne!(b, c);
    }

    #[test]
    fn crc16_stable_and_distinct() {
        let a = crc16_of_str("c:/foo/bar.jpg");
        let b = crc16_of_str("c:/foo/baz.jpg");
        let a2 = crc16_of_str("c:/foo/bar.jpg");
        assert_eq!(a, a2);
        assert_ne!(a, b);
    }

    #[test]
    fn panorama_state_resets_to_initial() {
        let mut s = PanoramaState::new(0.5, -0.2);
        s.yaw = 1.5;
        s.pitch = 0.3;
        s.fov_y = 0.5;
        s.drag_active = true;
        s.reset();
        assert_eq!(s.yaw, 0.5);
        assert_eq!(s.pitch, -0.2);
        assert_eq!(s.fov_y, FOV_DEFAULT);
        assert!(!s.drag_active);
    }

    #[test]
    fn pitch_clamped_in_new() {
        let s = PanoramaState::new(0.0, 100.0);
        assert!(s.pitch <= PITCH_LIMIT);
        let s = PanoramaState::new(0.0, -100.0);
        assert!(s.pitch >= -PITCH_LIMIT);
    }

    // ---- PanoUvTransform: 部分 FOV equirect (Phase 1.5) ----

    fn make_pano_info(
        full: Option<(u32, u32)>,
        cropped: Option<(u32, u32)>,
        left: Option<u32>,
        top: Option<u32>,
    ) -> crate::xmp_reader::XmpPanoramaInfo {
        crate::xmp_reader::XmpPanoramaInfo {
            projection_type: Some("equirectangular".to_string()),
            use_panorama_viewer: Some(true),
            full_pano_width_pixels: full.map(|(w, _)| w),
            full_pano_height_pixels: full.map(|(_, h)| h),
            cropped_area_image_width_pixels: cropped.map(|(w, _)| w),
            cropped_area_image_height_pixels: cropped.map(|(_, h)| h),
            cropped_area_left_pixels: left,
            cropped_area_top_pixels: top,
            pose_pitch_degrees: None,
            pose_heading_degrees: None,
            pose_roll_degrees: None,
        }
    }

    #[test]
    fn uv_transform_identity_when_full_equals_cropped() {
        // 完全フル equirect: cropped = full、offset 0
        let info = make_pano_info(Some((4096, 2048)), Some((4096, 2048)), Some(0), Some(0));
        let x = PanoUvTransform::from_gpano(&info).expect("should compute");
        assert!(x.is_identity(), "got {:?}", x);
    }

    #[test]
    fn uv_transform_partial_fov_dslr_example() {
        // 設計書 §11.2 の例: 15126×7562 のフル球面に対し 15126×5795 で水平全周だが垂直 77%
        // 中央寄せ (top = (7562 - 5795) / 2 ≒ 883)
        let info = make_pano_info(Some((15126, 7562)), Some((15126, 5795)), Some(0), Some(883));
        let x = PanoUvTransform::from_gpano(&info).expect("should compute");
        // 水平はフル覆い
        assert!((x.u_scale - 1.0).abs() < 0.001, "u_scale = {}", x.u_scale);
        assert_eq!(x.u_offset, 0.0);
        // 垂直は ~76.6%
        let expected_v_scale = 5795.0 / 7562.0;
        assert!((x.v_scale - expected_v_scale).abs() < 0.001);
        let expected_v_offset = 883.0 / 7562.0;
        assert!((x.v_offset - expected_v_offset).abs() < 0.001);
        assert!(!x.is_identity());
    }

    #[test]
    fn uv_transform_left_top_default_to_center() {
        // CroppedAreaLeftPixels / TopPixels が無い場合は中央寄せ
        let info = make_pano_info(Some((4096, 2048)), Some((2048, 1024)), None, None);
        let x = PanoUvTransform::from_gpano(&info).expect("should compute");
        // 中央寄せ: left = (4096 - 2048) / 2 = 1024、top = (2048 - 1024) / 2 = 512
        // u_offset = 1024/4096 = 0.25、v_offset = 512/2048 = 0.25
        assert!((x.u_offset - 0.25).abs() < 0.001);
        assert!((x.v_offset - 0.25).abs() < 0.001);
        assert!((x.u_scale - 0.5).abs() < 0.001);
        assert!((x.v_scale - 0.5).abs() < 0.001);
    }

    #[test]
    fn uv_transform_returns_none_when_required_missing() {
        // FullPanoWidth が無いと計算不可
        let info = make_pano_info(Some((0, 2048)), Some((4096, 2048)), Some(0), Some(0));
        assert!(PanoUvTransform::from_gpano(&info).is_none());

        let info = make_pano_info(None, Some((4096, 2048)), Some(0), Some(0));
        assert!(PanoUvTransform::from_gpano(&info).is_none());

        let info = make_pano_info(Some((4096, 2048)), None, Some(0), Some(0));
        assert!(PanoUvTransform::from_gpano(&info).is_none());
    }

    #[test]
    fn uv_transform_returns_none_when_cropped_exceeds_full() {
        // CroppedArea が FullPano をはみ出す → 防衛的に identity 化
        let info = make_pano_info(Some((4096, 2048)), Some((5000, 2048)), Some(0), Some(0));
        assert!(PanoUvTransform::from_gpano(&info).is_none());
    }

    #[test]
    fn uv_transform_near_full_snaps_to_identity() {
        // 浮動小数誤差程度の微差は identity に丸める (= 不要な UV 変換を避ける)
        let info = make_pano_info(Some((10000, 5000)), Some((9999, 4999)), Some(0), Some(0));
        let x = PanoUvTransform::from_gpano(&info).expect("should compute");
        assert!(x.is_identity());
    }
}
