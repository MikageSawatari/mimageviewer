//! 動画の球面 (360 度) メタデータ。静止画側の GPano XMP と同じ役割を果たす。
//!
//! 静止画は XMP の `GPano:*` を読んで 360 かどうかを決める
//! ([panorama.rs](../panorama.rs)、[panorama-360-view-plan.md §2](../../docs/panorama-360-view-plan.md))。
//! 動画は FFmpeg が `AV_PKT_DATA_SPHERICAL` (= `AVSphericalMapping`) と
//! `AV_PKT_DATA_STEREO3D` を stream side data として出すので、そこから同じ判断材料を作る。
//! 構造は [`display_metadata`](super::display_metadata) (回転の display matrix) と同じ:
//! **FFmpeg の生バイト列をこのモジュールだけで型に直し、上位には正規化済みの値を渡す。**
//!
//! ## 実素材を測って分かったこと (backlog §1.112、2026-08-24)
//!
//! 1. **メタデータを持つ実ファイルは少数派**。実在の 360 動画 10 件のうち、spherical
//!    メタデータがあったのは 2 件だけだった。したがって**メタデータ判定だけでは足りず**、
//!    静止画側と同じ 2:1 アスペクト比のフォールバックが要る ([`detect`] 参照)。
//! 2. **部分 FOV は別の enum になる**。`equi` の bounds が非ゼロだと FFmpeg は
//!    `EQUIRECTANGULAR` ではなく `EQUIRECTANGULAR_TILE` を返す。静止画側の GPano
//!    `CroppedArea*` に相当するので、同じ [`PanoUvTransform`] へ落とす。
//! 3. **上下分割ステレオ (3D 360) が実在する**。モノラル equirect として扱うと同じ絵が
//!    2 つ並んで見えるので、[`VideoStereoLayout`] を見て 360 表示の対象から外す。

use ffmpeg_the_third::codec::packet::side_data::Type as PacketSideDataType;

use crate::panorama::PanoUvTransform;

/// 16.16 固定小数 (`AVSphericalMapping` の yaw/pitch/roll) の分母。
const FIXED_16_16: f32 = 65_536.0;
/// 0.32 固定小数 (`AVSphericalMapping` の bound_*) の分母。
const FIXED_0_32: f64 = 4_294_967_296.0;

/// `AVSphericalMapping` のバイト長 (projection + yaw/pitch/roll + bounds 4 + padding)。
const SPHERICAL_SIDE_DATA_LEN: usize = 4 + 3 * 4 + 4 * 4 + 4;

/// `AVSphericalProjection` の値。**順序は FFmpeg の enum 定義そのもの**なので、
/// `vendor/ffmpeg/include/libavutil/spherical.h` を更新したらここも確認する。
const PROJECTION_EQUIRECTANGULAR: u32 = 0;
const PROJECTION_CUBEMAP: u32 = 1;
const PROJECTION_EQUIRECTANGULAR_TILE: u32 = 2;
const PROJECTION_HALF_EQUIRECTANGULAR: u32 = 3;
const PROJECTION_RECTILINEAR: u32 = 4;
const PROJECTION_FISHEYE: u32 = 5;

/// `AVStereo3DType` の値。
const STEREO3D_2D: u32 = 0;
const STEREO3D_SIDEBYSIDE: u32 = 1;
const STEREO3D_TOPBOTTOM: u32 = 2;

/// この動画がどの投影で球面を平面へ写しているか。
///
/// mIV が描けるのは equirectangular だけ。**それ以外を「未対応」として明示的に持つ**のは、
/// メタデータが「これは 360 だが cubemap だ」と言っている素材を、アスペクト比の弱い
/// シグナルで equirect として描いてしまわないため (GoPro Max の `.360` が実際に EAC)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoSphericalProjection {
    /// 全球 equirectangular。静止画側の「フル equirect」と同じ。
    Equirectangular,
    /// 部分 FOV equirectangular。bounds が球面上のどこを覆うかを表す。
    /// 静止画側の GPano `CroppedArea*` に対応する。
    EquirectangularTile,
    /// mIV が描けない投影 (cubemap / half equirect / rectilinear / fisheye / 未知)。
    /// 値は FFmpeg の enum をそのまま持つ (ログと将来対応の判断材料)。
    Unsupported(u32),
}

impl VideoSphericalProjection {
    fn from_raw(value: u32) -> Self {
        match value {
            PROJECTION_EQUIRECTANGULAR => Self::Equirectangular,
            PROJECTION_EQUIRECTANGULAR_TILE => Self::EquirectangularTile,
            other => Self::Unsupported(other),
        }
    }

    /// mIV の 360 ビューで描けるか。
    pub fn is_renderable(self) -> bool {
        matches!(self, Self::Equirectangular | Self::EquirectangularTile)
    }

    /// ログ / 診断用の短い名前。利用者向け UI には出さない。
    pub fn debug_name(self) -> &'static str {
        match self {
            Self::Equirectangular => "equirectangular",
            Self::EquirectangularTile => "equirectangular-tile",
            Self::Unsupported(PROJECTION_CUBEMAP) => "cubemap",
            Self::Unsupported(PROJECTION_HALF_EQUIRECTANGULAR) => "half-equirectangular",
            Self::Unsupported(PROJECTION_RECTILINEAR) => "rectilinear",
            Self::Unsupported(PROJECTION_FISHEYE) => "fisheye",
            Self::Unsupported(_) => "unknown",
        }
    }
}

/// ステレオ (3D) の並べ方。
///
/// 360 動画にはモノラルと、左右の目を上下 / 左右に並べたものがある。**後者をモノラル
/// equirect として描くと同じ絵が 2 つ見える**ので、360 表示の対象から外す判断に使う。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoStereoLayout {
    /// 単眼。360 表示してよい。
    Mono,
    /// 上下分割。
    TopBottom,
    /// 左右分割。
    SideBySide,
    /// その他 (frame sequential など)。値は FFmpeg の enum をそのまま持つ。
    Other(u32),
}

impl VideoStereoLayout {
    fn from_raw(value: u32) -> Self {
        match value {
            STEREO3D_2D => Self::Mono,
            STEREO3D_TOPBOTTOM => Self::TopBottom,
            STEREO3D_SIDEBYSIDE => Self::SideBySide,
            other => Self::Other(other),
        }
    }

    /// 単眼として扱ってよいか。
    pub fn is_mono(self) -> bool {
        matches!(self, Self::Mono)
    }
}

/// 動画ストリームから読んだ球面メタデータ (正規化済み)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VideoSphericalMapping {
    pub projection: VideoSphericalProjection,
    /// 初期視点 (度)。`AVSphericalMapping` の 16.16 固定小数を実数へ直したもの。
    /// 静止画側の GPano `PoseHeadingDegrees` / `PosePitchDegrees` に対応する。
    pub yaw_degrees: f32,
    pub pitch_degrees: f32,
    pub roll_degrees: f32,
    /// タイル (部分 FOV) の場合の球面上の位置。全球なら [`PanoUvTransform::IDENTITY`]。
    pub uv_transform: PanoUvTransform,
}

impl VideoSphericalMapping {
    /// FFmpeg の side data バイト列を解釈する。
    ///
    /// バイト順は `display_metadata` の display matrix と同じくネイティブエンディアン
    /// (FFmpeg が構造体をそのままコピーして出すため)。長さが足りなければ `None`。
    pub fn from_side_data_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < SPHERICAL_SIDE_DATA_LEN {
            return None;
        }
        let read_u32 = |offset: usize| -> u32 {
            u32::from_ne_bytes(data[offset..offset + 4].try_into().unwrap_or([0; 4]))
        };
        let read_i32 = |offset: usize| -> i32 {
            i32::from_ne_bytes(data[offset..offset + 4].try_into().unwrap_or([0; 4]))
        };

        let projection = VideoSphericalProjection::from_raw(read_u32(0));
        let yaw_degrees = read_i32(4) as f32 / FIXED_16_16;
        let pitch_degrees = read_i32(8) as f32 / FIXED_16_16;
        let roll_degrees = read_i32(12) as f32 / FIXED_16_16;
        let bound_left = read_u32(16);
        let bound_top = read_u32(20);
        let bound_right = read_u32(24);
        let bound_bottom = read_u32(28);

        // bounds は tile のときだけ意味を持つ (spherical.h の注記どおり)。
        let uv_transform = if projection == VideoSphericalProjection::EquirectangularTile {
            uv_transform_from_bounds(bound_left, bound_top, bound_right, bound_bottom)
                .unwrap_or(PanoUvTransform::IDENTITY)
        } else {
            PanoUvTransform::IDENTITY
        };

        Some(Self {
            projection,
            yaw_degrees,
            pitch_degrees,
            roll_degrees,
            uv_transform,
        })
    }
}

/// 0.32 固定小数の bounds を、静止画側と同じ UV 変換へ直す。
///
/// bounds は「各辺から切り落とされている割合」。covered 部分が UV 空間で占める範囲は
/// `offset = left`、`scale = 1 - left - right` (V も同様)。**静止画の
/// `PanoUvTransform::from_gpano` と同じ意味の値**になるので、シェーダ側は 1 つの経路で済む。
///
/// 全球 (全部ゼロ) や、計算が破綻する値 (合計が 1 以上 / 極端に小さい) では `None` を返し、
/// 呼び出し側は identity へ倒す。
fn uv_transform_from_bounds(
    bound_left: u32,
    bound_top: u32,
    bound_right: u32,
    bound_bottom: u32,
) -> Option<PanoUvTransform> {
    if bound_left == 0 && bound_top == 0 && bound_right == 0 && bound_bottom == 0 {
        return None;
    }
    let left = f64::from(bound_left) / FIXED_0_32;
    let top = f64::from(bound_top) / FIXED_0_32;
    let right = f64::from(bound_right) / FIXED_0_32;
    let bottom = f64::from(bound_bottom) / FIXED_0_32;
    let u_scale = 1.0 - left - right;
    let v_scale = 1.0 - top - bottom;
    // 覆う範囲が消える / 反転する値は壊れた宣言なので使わない。
    if !(1.0e-3..=1.0).contains(&u_scale) || !(1.0e-3..=1.0).contains(&v_scale) {
        return None;
    }
    Some(PanoUvTransform {
        u_offset: left as f32,
        v_offset: top as f32,
        u_scale: u_scale as f32,
        v_scale: v_scale as f32,
    })
}

/// 選択された映像ストリームから `AV_PKT_DATA_SPHERICAL` を読む。
pub fn spherical_from_stream(
    stream: &ffmpeg_the_third::format::stream::Stream<'_>,
) -> Option<VideoSphericalMapping> {
    stream
        .side_data()
        .find(|side_data| side_data.kind() == PacketSideDataType::DataSpherical)
        .and_then(|side_data| VideoSphericalMapping::from_side_data_bytes(side_data.data()))
}

/// 選択された映像ストリームから `AV_PKT_DATA_STEREO3D` を読む。
///
/// side data が無い動画は圧倒的多数なので、その場合は [`VideoStereoLayout::Mono`] を返す
/// (「宣言が無い = 単眼」が実態に合う)。
pub fn stereo_layout_from_stream(
    stream: &ffmpeg_the_third::format::stream::Stream<'_>,
) -> VideoStereoLayout {
    stream
        .side_data()
        .find(|side_data| side_data.kind() == PacketSideDataType::Stereo3d)
        .and_then(|side_data| {
            let data = side_data.data();
            if data.len() < 4 {
                return None;
            }
            Some(VideoStereoLayout::from_raw(u32::from_ne_bytes(
                data[0..4].try_into().ok()?,
            )))
        })
        .unwrap_or(VideoStereoLayout::Mono)
}

/// 360 表示を提案する強さ。静止画側の [`crate::panorama::PanoramaTrigger`] と同じ意味。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoPanoramaTrigger {
    /// メタデータが equirectangular だと明言している。
    Auto,
    /// メタデータは無いが、表示アスペクト比が 2:1 なので候補。
    Hint,
}

/// 360 表示の対象にしない理由。**黙って `None` を返さない**ための型。
///
/// 「なぜこの動画では 360 ボタンが出ないのか」を利用者へ説明でき、ログでも追える。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoPanoramaRejection {
    /// 球面メタデータはあるが、mIV が描けない投影 (cubemap 等)。
    UnsupportedProjection(VideoSphericalProjection),
    /// 左右 / 上下に 2 つの目が入っている。単眼として描くと二重像になる。
    Stereoscopic(VideoStereoLayout),
    /// メタデータも 2:1 も無い。ただの平面動画。
    NotPanoramic,
}

/// 動画が 360 ビューの対象かを決める。
///
/// 判定順とその理由:
/// 1. **ステレオが最優先**。equirect と宣言されていても、左右 2 つの目が入っていれば
///    単眼として描いた瞬間に二重像になる。
/// 2. **球面メタデータがあればそれに従う**。「360 だが cubemap」と分かっているものを、
///    アスペクト比の弱いシグナルで equirect として描かない。
/// 3. **メタデータが無いときだけ 2:1 へ落ちる**。実素材の大半がここに来る (§1.112 の実測)。
///
/// `display_width` / `display_height` は **表示上の寸法** (SAR と回転を反映した後) を渡す。
/// 生の符号化寸法で判定すると、回転メタデータ付きの縦持ち動画を取りこぼす。
pub fn detect(
    mapping: Option<&VideoSphericalMapping>,
    stereo: VideoStereoLayout,
    display_width: u32,
    display_height: u32,
) -> Result<VideoPanoramaTrigger, VideoPanoramaRejection> {
    if !stereo.is_mono() {
        return Err(VideoPanoramaRejection::Stereoscopic(stereo));
    }
    if let Some(mapping) = mapping {
        return if mapping.projection.is_renderable() {
            Ok(VideoPanoramaTrigger::Auto)
        } else {
            Err(VideoPanoramaRejection::UnsupportedProjection(
                mapping.projection,
            ))
        };
    }
    if display_height == 0 {
        return Err(VideoPanoramaRejection::NotPanoramic);
    }
    let aspect = display_width as f32 / display_height as f32;
    if (crate::panorama::ASPECT_LOW..=crate::panorama::ASPECT_HIGH).contains(&aspect) {
        Ok(VideoPanoramaTrigger::Hint)
    } else {
        Err(VideoPanoramaRejection::NotPanoramic)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `AVSphericalMapping` と同じ並びのバイト列を作る (native endian)。
    fn side_data(
        projection: u32,
        yaw: i32,
        pitch: i32,
        roll: i32,
        bounds: (u32, u32, u32, u32),
    ) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&projection.to_ne_bytes());
        out.extend_from_slice(&yaw.to_ne_bytes());
        out.extend_from_slice(&pitch.to_ne_bytes());
        out.extend_from_slice(&roll.to_ne_bytes());
        out.extend_from_slice(&bounds.0.to_ne_bytes());
        out.extend_from_slice(&bounds.1.to_ne_bytes());
        out.extend_from_slice(&bounds.2.to_ne_bytes());
        out.extend_from_slice(&bounds.3.to_ne_bytes());
        out.extend_from_slice(&0_u32.to_ne_bytes()); // padding
        out
    }

    #[test]
    fn a_full_sphere_declaration_parses_to_an_identity_transform() {
        let bytes = side_data(PROJECTION_EQUIRECTANGULAR, 0, 0, 0, (0, 0, 0, 0));
        let m = VideoSphericalMapping::from_side_data_bytes(&bytes).expect("parses");
        assert_eq!(m.projection, VideoSphericalProjection::Equirectangular);
        assert!(m.uv_transform.is_identity());
    }

    /// 16.16 固定小数の初期視点。静止画の GPano pose と同じ役割なので、度で取り出す。
    #[test]
    fn the_initial_pose_is_read_as_degrees() {
        let bytes = side_data(
            PROJECTION_EQUIRECTANGULAR,
            (90.0 * FIXED_16_16) as i32,
            (-30.0 * FIXED_16_16) as i32,
            (15.0 * FIXED_16_16) as i32,
            (0, 0, 0, 0),
        );
        let m = VideoSphericalMapping::from_side_data_bytes(&bytes).expect("parses");
        assert!((m.yaw_degrees - 90.0).abs() < 1.0e-3);
        assert!((m.pitch_degrees + 30.0).abs() < 1.0e-3);
        assert!((m.roll_degrees - 15.0).abs() < 1.0e-3);
    }

    /// 部分 FOV は静止画の `CroppedArea*` と同じ UV へ落ちる。上下 25% ずつ欠けた素材
    /// (テストセットの `spherical_meta_partial_fov`) がこの形。
    #[test]
    fn a_tile_declaration_becomes_the_same_uv_transform_the_still_side_uses() {
        let quarter = (0.25_f64 * FIXED_0_32) as u32;
        let bytes = side_data(
            PROJECTION_EQUIRECTANGULAR_TILE,
            0,
            0,
            0,
            (0, quarter, 0, quarter),
        );
        let m = VideoSphericalMapping::from_side_data_bytes(&bytes).expect("parses");
        assert_eq!(m.projection, VideoSphericalProjection::EquirectangularTile);
        assert!(!m.uv_transform.is_identity());
        assert!((m.uv_transform.v_offset - 0.25).abs() < 1.0e-4);
        assert!((m.uv_transform.v_scale - 0.5).abs() < 1.0e-4);
        assert!((m.uv_transform.u_scale - 1.0).abs() < 1.0e-6);
        // 垂直だけの crop では U の seam wrap を保つ (静止画側と同じ判定)。
        assert!(!m.uv_transform.has_horizontal_crop());
    }

    /// bounds が全球以外を指していないタイル宣言は identity へ倒す。
    #[test]
    fn a_tile_declaration_without_bounds_falls_back_to_identity() {
        let bytes = side_data(PROJECTION_EQUIRECTANGULAR_TILE, 0, 0, 0, (0, 0, 0, 0));
        let m = VideoSphericalMapping::from_side_data_bytes(&bytes).expect("parses");
        assert!(m.uv_transform.is_identity());
    }

    /// 壊れた bounds (覆う範囲が消える) で UV がゼロ除算側へ倒れないこと。
    #[test]
    fn broken_bounds_do_not_produce_a_degenerate_transform() {
        let full = u32::MAX;
        assert!(uv_transform_from_bounds(full, 0, full, 0).is_none());
        assert!(uv_transform_from_bounds(0, full, 0, full).is_none());
    }

    /// bounds は tile のときだけ意味を持つ (spherical.h の注記)。全球宣言に紛れ込んだ
    /// 値を UV へ反映すると、全球の素材が部分 FOV として歪む。
    #[test]
    fn bounds_are_ignored_unless_the_projection_is_a_tile() {
        let quarter = (0.25_f64 * FIXED_0_32) as u32;
        let bytes = side_data(
            PROJECTION_EQUIRECTANGULAR,
            0,
            0,
            0,
            (0, quarter, 0, quarter),
        );
        let m = VideoSphericalMapping::from_side_data_bytes(&bytes).expect("parses");
        assert!(m.uv_transform.is_identity());
    }

    #[test]
    fn short_side_data_is_rejected() {
        assert!(VideoSphericalMapping::from_side_data_bytes(&[0_u8; 8]).is_none());
    }

    #[test]
    fn unsupported_projections_keep_their_raw_value() {
        for (raw, name) in [
            (PROJECTION_CUBEMAP, "cubemap"),
            (PROJECTION_HALF_EQUIRECTANGULAR, "half-equirectangular"),
            (PROJECTION_RECTILINEAR, "rectilinear"),
            (PROJECTION_FISHEYE, "fisheye"),
            (99, "unknown"),
        ] {
            let p = VideoSphericalProjection::from_raw(raw);
            assert_eq!(p, VideoSphericalProjection::Unsupported(raw));
            assert!(!p.is_renderable());
            assert_eq!(p.debug_name(), name);
        }
    }

    // ---- detect ----

    fn equirect() -> VideoSphericalMapping {
        VideoSphericalMapping {
            projection: VideoSphericalProjection::Equirectangular,
            yaw_degrees: 0.0,
            pitch_degrees: 0.0,
            roll_degrees: 0.0,
            uv_transform: PanoUvTransform::IDENTITY,
        }
    }

    #[test]
    fn a_declared_equirectangular_video_is_detected_automatically() {
        assert_eq!(
            detect(Some(&equirect()), VideoStereoLayout::Mono, 3840, 2048),
            Ok(VideoPanoramaTrigger::Auto),
            "the declaration wins even when the frame is not 2:1"
        );
    }

    /// **実素材の大半はここに来る。** メタデータが無くても 2:1 なら候補にする。
    #[test]
    fn a_two_to_one_video_without_metadata_is_still_offered() {
        assert_eq!(
            detect(None, VideoStereoLayout::Mono, 3840, 1920),
            Ok(VideoPanoramaTrigger::Hint)
        );
        assert_eq!(
            detect(None, VideoStereoLayout::Mono, 1920, 960),
            Ok(VideoPanoramaTrigger::Hint)
        );
    }

    /// 2:1 でも metadata でもないものは平面動画。テストセットの `non21_*` がこれ。
    #[test]
    fn an_ordinary_video_is_not_offered() {
        assert_eq!(
            detect(None, VideoStereoLayout::Mono, 1920, 1080),
            Err(VideoPanoramaRejection::NotPanoramic)
        );
        assert_eq!(
            detect(None, VideoStereoLayout::Mono, 3840, 2048),
            Err(VideoPanoramaRejection::NotPanoramic),
            "1.875 is outside the still side's 2:1 tolerance"
        );
    }

    /// GoPro Max の `.360` (EAC) のように「360 だが描けない投影」は、
    /// **アスペクト比のフォールバックへ落とさない**。落とすと別物として描いてしまう。
    #[test]
    fn a_declared_but_unsupported_projection_does_not_fall_back_to_the_aspect_hint() {
        let cubemap = VideoSphericalMapping {
            projection: VideoSphericalProjection::Unsupported(PROJECTION_CUBEMAP),
            ..equirect()
        };
        assert_eq!(
            detect(Some(&cubemap), VideoStereoLayout::Mono, 3840, 1920),
            Err(VideoPanoramaRejection::UnsupportedProjection(
                VideoSphericalProjection::Unsupported(PROJECTION_CUBEMAP)
            )),
            "a 2:1 cubemap must not be drawn as equirectangular"
        );
    }

    /// ステレオは投影の宣言より優先して弾く。単眼として描くと二重像になる。
    #[test]
    fn stereoscopic_video_is_rejected_before_the_projection_is_considered() {
        for layout in [
            VideoStereoLayout::TopBottom,
            VideoStereoLayout::SideBySide,
            VideoStereoLayout::Other(7),
        ] {
            assert_eq!(
                detect(Some(&equirect()), layout, 3840, 1920),
                Err(VideoPanoramaRejection::Stereoscopic(layout)),
                "{layout:?} must not be drawn as a mono sphere"
            );
        }
    }

    /// 宣言が無い動画は単眼として扱う (圧倒的多数がこれ)。
    #[test]
    fn a_video_without_a_stereo_declaration_counts_as_mono() {
        assert!(VideoStereoLayout::from_raw(STEREO3D_2D).is_mono());
        assert!(!VideoStereoLayout::from_raw(STEREO3D_TOPBOTTOM).is_mono());
        assert!(!VideoStereoLayout::from_raw(STEREO3D_SIDEBYSIDE).is_mono());
    }

    /// 表示寸法がゼロのフレームで判定に入っても落ちない。
    #[test]
    fn zero_sized_video_is_rejected_without_dividing_by_zero() {
        assert_eq!(
            detect(None, VideoStereoLayout::Mono, 1920, 0),
            Err(VideoPanoramaRejection::NotPanoramic)
        );
    }
}
