//! EXIF メタデータ読み取り。
//!
//! JPEG/TIFF 等の画像ファイルから撮影情報 (カメラ, レンズ, 露出, GPS 等) を抽出する。
//! `rexif` crate を使用（`kamadak-exif` より寛容なパーサー）。

use std::path::Path;

/// EXIF タグの意味的グループ。
/// メタデータパネルのセクション分けと、preferences の非表示タグ設定 UI が共有する。
/// 仕様 (IFD) ではなく「ユーザーが何を見たいか / 隠したいか」で分類している。
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum TagGroup {
    Camera,   // カメラ / レンズ
    Shooting, // 撮影設定 (露出・絞り・ISO 等)
    Image,    // 画像情報 (サイズ・日時・色空間 等)
    Gps,      // GPS / 位置情報
    Other,    // その他 (拡張・Windows・環境センサー 等)
}

impl TagGroup {
    /// セクション見出しに使う日本語表記
    pub fn display_name(self) -> &'static str {
        match self {
            TagGroup::Camera => "カメラ",
            TagGroup::Shooting => "撮影設定",
            TagGroup::Image => "画像情報",
            TagGroup::Gps => "GPS",
            TagGroup::Other => "その他 / 拡張",
        }
    }

    /// 表示順 (preferences と panel で共通)
    pub const fn ordered() -> &'static [TagGroup] {
        &[
            TagGroup::Camera,
            TagGroup::Shooting,
            TagGroup::Image,
            TagGroup::Gps,
            TagGroup::Other,
        ]
    }
}

/// レジストリ 1 行。canonical 名 + 日本語表示名 + グループ。
pub struct TagInfo {
    pub name: &'static str,
    pub display: &'static str,
    pub group: TagGroup,
}

/// 抽出した EXIF 情報
#[derive(Clone, Debug, Default)]
pub struct ExifInfo {
    /// セクションごとにまとめた (グループ, [(タグ名, 値)]) のリスト
    pub sections: Vec<(TagGroup, Vec<(String, String)>)>,
}

/// ファイルから EXIF 情報を読み取る。EXIF が無い場合は None。
/// `hidden_tags` に含まれるタグ名は結果から除外する。
pub fn read_exif(path: &Path, hidden_tags: &[String]) -> Option<ExifInfo> {
    let exif = rexif::parse_file(path.to_str()?).ok()?;
    build_exif_info(&exif.entries, hidden_tags)
}

/// バイト列から EXIF 情報を読み取る（ZIP 内画像用）。
pub fn read_exif_from_bytes(bytes: &[u8], hidden_tags: &[String]) -> Option<ExifInfo> {
    let exif = rexif::parse_buffer(bytes).ok()?;
    build_exif_info(&exif.entries, hidden_tags)
}

fn build_exif_info(entries: &[rexif::ExifEntry], hidden_tags: &[String]) -> Option<ExifInfo> {
    if entries.is_empty() {
        return None;
    }

    use std::collections::HashMap;
    let mut buckets: HashMap<TagGroup, Vec<(String, String)>> = HashMap::new();

    for e in entries {
        // 構造タグ (IFD へのオフセット / バイナリ blob) は常に抑止。
        // ユーザーにとって意味がなく、設定での hide 操作の対象にもしない。
        if is_structural(e) {
            continue;
        }

        let tag_name = canonical_tag_name(e);

        // 非表示タグはスキップ
        if hidden_tags.iter().any(|h| h == &tag_name) {
            continue;
        }

        let value = &e.value_more_readable;

        // 空値をスキップ
        if value.is_empty() || value.trim().is_empty() {
            continue;
        }
        // SubIFD ポインタ等の内部情報をスキップ
        if value.contains("byte offset") || value.starts_with("Blob of ") {
            continue;
        }
        // 未認識タグの生値 ([tag=xxxx] で始まる場合) で役に立たないものをスキップ
        if value.starts_with("[tag=") && tag_name.starts_with("Tag(") {
            continue;
        }

        // rexif の value に [tag=xxxx] プレフィクスが付くことがある → 除去
        let mut display = if value.starts_with("[tag=") {
            if let Some(end) = value.find("] ") {
                value[end + 2..].to_string()
            } else {
                value.to_string()
            }
        } else {
            value.to_string()
        };

        // タグ固有の整形 (バージョン文字列など)
        display = format_value(e.ifd.tag, &display);

        let entry = (tag_name.clone(), display);

        // GPS IFD は IFD 種別で確実に判定する (GPS タグ ID は他 IFD と衝突するため)。
        // それ以外は canonical 名から TAG_REGISTRY を引いて group を決める。
        let group = if e.kind == rexif::IfdKind::Gps {
            TagGroup::Gps
        } else {
            tag_group(&tag_name)
        };
        buckets.entry(group).or_default().push(entry);
    }

    // TagGroup::ordered() で固定順に並べる
    let mut sections = Vec::new();
    for &g in TagGroup::ordered() {
        if let Some(fields) = buckets.remove(&g) {
            if !fields.is_empty() {
                sections.push((g, fields));
            }
        }
    }

    if sections.is_empty() {
        None
    } else {
        Some(ExifInfo { sections })
    }
}

/// タグ ID に応じて、rexif の生値をより読みやすい形に整える。
/// 既知の整形パターンだけ上書きし、それ以外はそのまま返す。
fn format_value(tag_id: u16, raw: &str) -> String {
    match tag_id {
        // Orientation は rexif 0.7.5 が 2/4/5/7 を "Unknown (0112=N)"
        // と表示するため、数値を拾ってユーザー向け表示に補完する。
        274 => format_orientation_value(raw),
        // 36864 ExifVersion / 40960 FlashpixVersion: 4 文字 ASCII "0231" → "2.31"
        // (先頭 '0' パディング + major 1 桁 + minor 2 桁)
        36864 | 40960 => {
            let digits: Vec<char> = raw.chars().filter(|c| c.is_ascii_digit()).collect();
            if digits.len() == 4 {
                let minor: String = digits[2..].iter().collect();
                format!("{}.{}", digits[1], minor)
            } else {
                raw.to_string()
            }
        }
        _ => raw.to_string(),
    }
}

fn format_orientation_value(raw: &str) -> String {
    let orientation = raw
        .trim()
        .parse::<u16>()
        .ok()
        .or_else(|| {
            raw.split_once('=')
                .and_then(|(_, rest)| rest.trim_end_matches(')').trim().parse::<u16>().ok())
        })
        .or_else(|| {
            let lower = raw.to_lowercase();
            if lower.contains("straight") || lower.contains("normal") {
                Some(1)
            } else if lower.contains("upside down") || lower.contains("180") {
                Some(3)
            } else if lower.contains("rotated to left") || lower.contains("90 cw") {
                Some(6)
            } else if lower.contains("rotated to right")
                || lower.contains("270 cw")
                || lower.contains("90 ccw")
            {
                Some(8)
            } else {
                None
            }
        });

    match orientation {
        Some(1) => "標準 (1)".to_string(),
        Some(2) => "左右反転 (2)".to_string(),
        Some(3) => "180度回転 (3)".to_string(),
        Some(4) => "上下反転 (4)".to_string(),
        Some(5) => "左右反転 + 90度回転 (5)".to_string(),
        Some(6) => "90度回転 (6)".to_string(),
        Some(7) => "左右反転 + 270度回転 (7)".to_string(),
        Some(8) => "270度回転 (8)".to_string(),
        _ => raw.to_string(),
    }
}

/// 構造的な「IFD オフセット / 内部バイナリ」タグかどうか。
///
/// これらはファイル形式のプラミングで、ユーザー表示上の意味が無い。
/// rexif の typed enum で拾えるものは enum で、拾えないもの (InteropIFDPointer)
/// は raw ID で抑止する。
fn is_structural(e: &rexif::ExifEntry) -> bool {
    use rexif::ExifTag;
    if matches!(
        e.tag,
        ExifTag::ExifOffset | ExifTag::GPSOffset | ExifTag::MakerNote
    ) {
        return true;
    }
    matches!(e.ifd.tag, 40965 /* InteropIFDPointer */)
}

/// EXIF エントリの canonical タグ名を返す。
///
/// rexif の `ExifTag` enum (~70タグ網羅) に該当すれば typed match で名前を返し、
/// そうでなければ IFD 種別 + raw ID から補助テーブルで解決する。
/// canonical 名は本プロジェクトの命名 (空白なしの PascalCase) で固定し、
/// `default_exif_hidden_tags` の hide フィルタおよび `tag_display_name` の
/// 日本語マッピングと突き合わせる前提。
fn canonical_tag_name(e: &rexif::ExifEntry) -> String {
    use rexif::ExifTag;
    match e.tag {
        // 0th IFD
        ExifTag::ImageDescription => "ImageDescription".into(),
        ExifTag::Make => "Make".into(),
        ExifTag::Model => "Model".into(),
        ExifTag::Orientation => "Orientation".into(),
        ExifTag::XResolution => "XResolution".into(),
        ExifTag::YResolution => "YResolution".into(),
        ExifTag::ResolutionUnit => "ResolutionUnit".into(),
        ExifTag::Software => "Software".into(),
        ExifTag::DateTime => "DateTime".into(),
        ExifTag::HostComputer => "HostComputer".into(),
        ExifTag::WhitePoint => "WhitePoint".into(),
        ExifTag::PrimaryChromaticities => "PrimaryChromaticities".into(),
        ExifTag::YCbCrCoefficients => "YCbCrCoefficients".into(),
        ExifTag::ReferenceBlackWhite => "ReferenceBlackWhite".into(),
        ExifTag::Copyright => "Copyright".into(),

        // Exif Sub IFD
        ExifTag::ExposureTime => "ExposureTime".into(),
        ExifTag::FNumber => "FNumber".into(),
        ExifTag::ExposureProgram => "ExposureProgram".into(),
        ExifTag::SpectralSensitivity => "SpectralSensitivity".into(),
        // EXIF 2.3 で ISOSpeedRatings → PhotographicSensitivity に改名
        ExifTag::ISOSpeedRatings => "PhotographicSensitivity".into(),
        ExifTag::OECF => "OECF".into(),
        ExifTag::SensitivityType => "SensitivityType".into(),
        ExifTag::ExifVersion => "ExifVersion".into(),
        ExifTag::DateTimeOriginal => "DateTimeOriginal".into(),
        ExifTag::DateTimeDigitized => "DateTimeDigitized".into(),
        ExifTag::ShutterSpeedValue => "ShutterSpeedValue".into(),
        ExifTag::ApertureValue => "ApertureValue".into(),
        ExifTag::BrightnessValue => "BrightnessValue".into(),
        ExifTag::ExposureBiasValue => "ExposureBiasValue".into(),
        ExifTag::MaxApertureValue => "MaxApertureValue".into(),
        ExifTag::SubjectDistance => "SubjectDistance".into(),
        ExifTag::MeteringMode => "MeteringMode".into(),
        ExifTag::LightSource => "LightSource".into(),
        ExifTag::Flash => "Flash".into(),
        ExifTag::FocalLength => "FocalLength".into(),
        ExifTag::SubjectArea => "SubjectArea".into(),
        ExifTag::UserComment => "UserComment".into(),
        ExifTag::FlashPixVersion => "FlashpixVersion".into(),
        ExifTag::ColorSpace => "ColorSpace".into(),
        ExifTag::RelatedSoundFile => "RelatedSoundFile".into(),
        ExifTag::FlashEnergy => "FlashEnergy".into(),
        ExifTag::FocalPlaneXResolution => "FocalPlaneXResolution".into(),
        ExifTag::FocalPlaneYResolution => "FocalPlaneYResolution".into(),
        ExifTag::FocalPlaneResolutionUnit => "FocalPlaneResolutionUnit".into(),
        ExifTag::SubjectLocation => "SubjectLocation".into(),
        ExifTag::ExposureIndex => "ExposureIndex".into(),
        ExifTag::SensingMethod => "SensingMethod".into(),
        ExifTag::FileSource => "FileSource".into(),
        ExifTag::SceneType => "SceneType".into(),
        ExifTag::CFAPattern => "CFAPattern".into(),
        ExifTag::CustomRendered => "CustomRendered".into(),
        ExifTag::ExposureMode => "ExposureMode".into(),
        // rexif は WhiteBalanceMode 名だが、当プロジェクトは WhiteBalance を採用
        ExifTag::WhiteBalanceMode => "WhiteBalance".into(),
        ExifTag::DigitalZoomRatio => "DigitalZoomRatio".into(),
        ExifTag::FocalLengthIn35mmFilm => "FocalLengthIn35mmFilm".into(),
        ExifTag::SceneCaptureType => "SceneCaptureType".into(),
        ExifTag::GainControl => "GainControl".into(),
        ExifTag::Contrast => "Contrast".into(),
        ExifTag::Saturation => "Saturation".into(),
        ExifTag::Sharpness => "Sharpness".into(),
        ExifTag::DeviceSettingDescription => "DeviceSettingDescription".into(),
        ExifTag::SubjectDistanceRange => "SubjectDistanceRange".into(),
        ExifTag::ImageUniqueID => "ImageUniqueID".into(),
        ExifTag::LensSpecification => "LensSpecification".into(),
        ExifTag::LensMake => "LensMake".into(),
        ExifTag::LensModel => "LensModel".into(),
        ExifTag::Gamma => "Gamma".into(),

        // GPS IFD
        ExifTag::GPSVersionID => "GPSVersionID".into(),
        ExifTag::GPSLatitudeRef => "GPSLatitudeRef".into(),
        ExifTag::GPSLatitude => "GPSLatitude".into(),
        ExifTag::GPSLongitudeRef => "GPSLongitudeRef".into(),
        ExifTag::GPSLongitude => "GPSLongitude".into(),
        ExifTag::GPSAltitudeRef => "GPSAltitudeRef".into(),
        ExifTag::GPSAltitude => "GPSAltitude".into(),
        ExifTag::GPSTimeStamp => "GPSTimeStamp".into(),
        ExifTag::GPSSatellites => "GPSSatellites".into(),
        ExifTag::GPSStatus => "GPSStatus".into(),
        ExifTag::GPSMeasureMode => "GPSMeasureMode".into(),
        ExifTag::GPSDOP => "GPSDOP".into(),
        ExifTag::GPSSpeedRef => "GPSSpeedRef".into(),
        ExifTag::GPSSpeed => "GPSSpeed".into(),
        ExifTag::GPSTrackRef => "GPSTrackRef".into(),
        ExifTag::GPSTrack => "GPSTrack".into(),
        ExifTag::GPSImgDirectionRef => "GPSImgDirectionRef".into(),
        ExifTag::GPSImgDirection => "GPSImgDirection".into(),
        ExifTag::GPSMapDatum => "GPSMapDatum".into(),
        ExifTag::GPSDestLatitudeRef => "GPSDestLatitudeRef".into(),
        ExifTag::GPSDestLatitude => "GPSDestLatitude".into(),
        ExifTag::GPSDestLongitudeRef => "GPSDestLongitudeRef".into(),
        ExifTag::GPSDestLongitude => "GPSDestLongitude".into(),
        ExifTag::GPSDestBearingRef => "GPSDestBearingRef".into(),
        ExifTag::GPSDestBearing => "GPSDestBearing".into(),
        ExifTag::GPSDestDistanceRef => "GPSDestDistanceRef".into(),
        ExifTag::GPSDestDistance => "GPSDestDistance".into(),
        ExifTag::GPSProcessingMethod => "GPSProcessingMethod".into(),
        ExifTag::GPSAreaInformation => "GPSAreaInformation".into(),
        ExifTag::GPSDateStamp => "GPSDateStamp".into(),
        ExifTag::GPSDifferential => "GPSDifferential".into(),

        // 構造系: is_structural で先に弾かれる想定 (到達した場合のセーフネット)
        ExifTag::ExifOffset | ExifTag::GPSOffset | ExifTag::MakerNote => String::new(),

        // rexif が認識しなかったタグ → 補助テーブルへ
        ExifTag::UnknownToMe => name_for_unknown_tag(e.kind, e.ifd.tag),
    }
}

/// rexif の `ExifTag` enum に無いタグを `(IFD種別, 生 ID)` で解決する補助テーブル。
/// EXIF 2.31/2.32 で追加されたタグ、Windows XP 拡張、TIFF/EP 等を補完する。
/// 新しいタグが「Tag(NNNN)」で出てきたら ExifTool 等で正体を確認してここに追加する。
fn name_for_unknown_tag(kind: rexif::IfdKind, raw: u16) -> String {
    use rexif::IfdKind;
    match (kind, raw) {
        // ── TIFF Rev 6.0 (0th IFD) で rexif が拾わないもの ──
        (_, 256) => "ImageWidth".into(),
        (_, 257) => "ImageLength".into(),
        (_, 274) => "Orientation".into(),
        (_, 282) => "XResolution".into(),
        (_, 283) => "YResolution".into(),
        (_, 296) => "ResolutionUnit".into(),
        (_, 305) => "Software".into(),
        (_, 306) => "DateTime".into(),
        (_, 315) => "Artist".into(),
        (_, 33432) => "Copyright".into(),
        // YCbCrPositioning など低価値タグも名前は付けておく (デフォ非表示)
        (_, 531) => "YCbCrPositioning".into(),

        // ── Exif Sub IFD で rexif が拾わないもの ──
        (_, 33434) => "ExposureTime".into(),
        (_, 33437) => "FNumber".into(),
        (_, 34850) => "ExposureProgram".into(),
        (_, 34855) => "PhotographicSensitivity".into(),
        (_, 34856) => "OECF".into(),
        (_, 34858) => "SensitivityType".into(),
        (_, 34859) => "StandardOutputSensitivity".into(),
        (_, 34860) => "RecommendedExposureIndex".into(),
        (_, 34861) => "ISOSpeed".into(),
        (_, 34862) => "ISOSpeedLatitudeyyy".into(),
        (_, 34863) => "ISOSpeedLatitudezzz".into(),
        (_, 34864) => "SensitivityType".into(),
        (_, 36864) => "ExifVersion".into(),
        // EXIF 2.31 の時刻タイムゾーン
        (_, 36880) => "OffsetTime".into(),
        (_, 36881) => "OffsetTimeOriginal".into(),
        (_, 36882) => "OffsetTimeDigitized".into(),
        (_, 37121) => "ComponentsConfiguration".into(),
        (_, 37122) => "CompressedBitsPerPixel".into(),
        (_, 37520) => "SubSecTime".into(),
        (_, 37521) => "SubSecTimeOriginal".into(),
        (_, 37522) => "SubSecTimeDigitized".into(),
        // EXIF 2.31 環境センサー
        (_, 37888) => "Temperature".into(),
        (_, 37889) => "Humidity".into(),
        (_, 37890) => "Pressure".into(),
        (_, 37891) => "WaterDepth".into(),
        (_, 37892) => "Acceleration".into(),
        (_, 37893) => "CameraElevationAngle".into(),
        // Microsoft Windows 拡張
        (_, 40091) => "XPTitle".into(),
        (_, 40092) => "XPComment".into(),
        (_, 40093) => "XPAuthor".into(),
        (_, 40094) => "XPKeywords".into(),
        (_, 40095) => "XPSubject".into(),
        (_, 40960) => "FlashpixVersion".into(),
        (_, 40962) => "PixelXDimension".into(),
        (_, 40963) => "PixelYDimension".into(),
        (_, 41484) => "SpatialFrequencyResponse".into(),
        (_, 41985) => "CustomRendered".into(),
        (_, 41995) => "DeviceSettingDescription".into(),
        (_, 42016) => "ImageUniqueID".into(),
        (_, 42032) => "CameraOwnerName".into(),
        (_, 42033) => "BodySerialNumber".into(),
        (_, 42037) => "LensSerialNumber".into(),
        (_, 42080) => "CompositeImage".into(),
        (_, 42240) => "Gamma".into(),

        // ── Interop IFD ──
        (IfdKind::Interoperability, 1) => "InteroperabilityIndex".into(),
        (IfdKind::Interoperability, 2) => "InteroperabilityVersion".into(),

        // ── GPS IFD: rexif の enum に無い拡張 ──
        (IfdKind::Gps, 31) => "GPSHPositioningError".into(),

        _ => format!("Tag({})", raw),
    }
}

/// 既知タグの登録テーブル。**Single source of truth**。
///
/// このテーブルから:
/// - [`tag_display_name`] (canonical name → 日本語)
/// - [`tag_group`] (canonical name → グループ)
/// - [`known_tags_in_group`] (preferences UI のグループ別チェックリスト用)
///
/// 新タグを追加するときはここに 1 行足すだけでよい。グルーピングは
/// [`TagGroup`] のドキュメント参照。
const TAG_REGISTRY: &[TagInfo] = &[
    // ── カメラ / レンズ ──
    TagInfo {
        name: "Make",
        display: "カメラ メーカー",
        group: TagGroup::Camera,
    },
    TagInfo {
        name: "Model",
        display: "カメラ モデル",
        group: TagGroup::Camera,
    },
    TagInfo {
        name: "Software",
        display: "プログラム名",
        group: TagGroup::Camera,
    },
    TagInfo {
        name: "HostComputer",
        display: "ホスト コンピュータ",
        group: TagGroup::Camera,
    },
    TagInfo {
        name: "CameraOwnerName",
        display: "カメラ所有者名",
        group: TagGroup::Camera,
    },
    TagInfo {
        name: "BodySerialNumber",
        display: "カメラ製造番号",
        group: TagGroup::Camera,
    },
    TagInfo {
        name: "LensMake",
        display: "レンズ メーカー",
        group: TagGroup::Camera,
    },
    TagInfo {
        name: "LensModel",
        display: "レンズ モデル",
        group: TagGroup::Camera,
    },
    TagInfo {
        name: "LensSerialNumber",
        display: "レンズ製造番号",
        group: TagGroup::Camera,
    },
    TagInfo {
        name: "LensSpecification",
        display: "レンズ スペック",
        group: TagGroup::Camera,
    },
    // ── 撮影設定 ──
    TagInfo {
        name: "ExposureTime",
        display: "露出時間",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "FNumber",
        display: "絞り値",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "ExposureProgram",
        display: "露出プログラム",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "SpectralSensitivity",
        display: "分光感度",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "PhotographicSensitivity",
        display: "ISO 速度",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "OECF",
        display: "OECF",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "SensitivityType",
        display: "感度種別",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "StandardOutputSensitivity",
        display: "標準出力感度",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "RecommendedExposureIndex",
        display: "推奨露出指数",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "ISOSpeed",
        display: "ISO スピード",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "ISOSpeedLatitudeyyy",
        display: "ISO スピード Latitude yyy",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "ISOSpeedLatitudezzz",
        display: "ISO スピード Latitude zzz",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "ShutterSpeedValue",
        display: "シャッタースピード",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "ApertureValue",
        display: "絞り値 (APEX)",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "BrightnessValue",
        display: "輝度値",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "ExposureBiasValue",
        display: "露出補正",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "MaxApertureValue",
        display: "最大絞り",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "SubjectDistance",
        display: "被写体距離",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "MeteringMode",
        display: "測光モード",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "LightSource",
        display: "光源",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "Flash",
        display: "フラッシュ モード",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "FocalLength",
        display: "焦点距離",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "FocalLengthIn35mmFilm",
        display: "35mm 焦点距離",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "FocalPlaneXResolution",
        display: "焦点面 X 解像度",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "FocalPlaneYResolution",
        display: "焦点面 Y 解像度",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "FocalPlaneResolutionUnit",
        display: "焦点面解像度の単位",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "SubjectArea",
        display: "被写体領域",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "SubjectLocation",
        display: "被写体位置",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "SubjectDistanceRange",
        display: "被写体距離レンジ",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "ExposureIndex",
        display: "露出指数",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "SensingMethod",
        display: "撮像方式",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "ExposureMode",
        display: "露出モード",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "WhiteBalance",
        display: "ホワイト バランス",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "DigitalZoomRatio",
        display: "デジタル ズーム",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "SceneCaptureType",
        display: "撮影シーン",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "GainControl",
        display: "ゲイン制御",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "Contrast",
        display: "コントラスト",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "Saturation",
        display: "彩度",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "Sharpness",
        display: "シャープネス",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "DeviceSettingDescription",
        display: "デバイス設定情報",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "FlashEnergy",
        display: "フラッシュ強度",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "SpatialFrequencyResponse",
        display: "空間周波数応答",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "Temperature",
        display: "温度",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "Humidity",
        display: "湿度",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "Pressure",
        display: "気圧",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "WaterDepth",
        display: "水深",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "Acceleration",
        display: "加速度",
        group: TagGroup::Shooting,
    },
    TagInfo {
        name: "CameraElevationAngle",
        display: "カメラ仰角",
        group: TagGroup::Shooting,
    },
    // ── 画像情報 ──
    TagInfo {
        name: "ImageDescription",
        display: "画像の説明",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "ImageWidth",
        display: "画像の幅",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "ImageLength",
        display: "画像の高さ",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "PixelXDimension",
        display: "幅 (pixel)",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "PixelYDimension",
        display: "高さ (pixel)",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "Orientation",
        display: "向き",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "XResolution",
        display: "水平方向の解像度",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "YResolution",
        display: "垂直方向の解像度",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "ResolutionUnit",
        display: "解像度の単位",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "DateTime",
        display: "変更日時",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "DateTimeOriginal",
        display: "撮影日時",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "DateTimeDigitized",
        display: "取得日時",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "OffsetTime",
        display: "時差",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "OffsetTimeOriginal",
        display: "撮影時の時差",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "OffsetTimeDigitized",
        display: "デジタル化時の時差",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "SubSecTime",
        display: "秒以下の時刻",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "SubSecTimeOriginal",
        display: "秒以下の撮影時刻",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "SubSecTimeDigitized",
        display: "秒以下のデジタル化時刻",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "ColorSpace",
        display: "色空間",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "Gamma",
        display: "ガンマ",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "WhitePoint",
        display: "白色点",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "PrimaryChromaticities",
        display: "原色色度",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "YCbCrCoefficients",
        display: "YCbCr 係数",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "YCbCrPositioning",
        display: "YCbCr 配置",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "ReferenceBlackWhite",
        display: "基準白黒点",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "Artist",
        display: "作成者",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "Copyright",
        display: "著作権",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "ImageUniqueID",
        display: "画像固有 ID",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "FileSource",
        display: "ファイル ソース",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "SceneType",
        display: "シーン タイプ",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "CFAPattern",
        display: "CFA パターン",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "CustomRendered",
        display: "カスタム レンダリング",
        group: TagGroup::Image,
    },
    TagInfo {
        name: "CompositeImage",
        display: "合成画像",
        group: TagGroup::Image,
    },
    // ── GPS ──
    TagInfo {
        name: "GPSVersionID",
        display: "GPS バージョン",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSLatitudeRef",
        display: "緯度基準",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSLatitude",
        display: "緯度",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSLongitudeRef",
        display: "経度基準",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSLongitude",
        display: "経度",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSAltitudeRef",
        display: "高度基準",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSAltitude",
        display: "高度",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSTimeStamp",
        display: "GPS 時刻",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSSatellites",
        display: "GPS 衛星",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSStatus",
        display: "GPS 受信状態",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSMeasureMode",
        display: "GPS 測位モード",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSDOP",
        display: "GPS 測位精度 (DOP)",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSSpeedRef",
        display: "速度単位",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSSpeed",
        display: "移動速度",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSTrackRef",
        display: "進行方向基準",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSTrack",
        display: "進行方向",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSImgDirectionRef",
        display: "撮影方位基準",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSImgDirection",
        display: "撮影方位",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSMapDatum",
        display: "測地系",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSDestLatitudeRef",
        display: "目的地 緯度基準",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSDestLatitude",
        display: "目的地 緯度",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSDestLongitudeRef",
        display: "目的地 経度基準",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSDestLongitude",
        display: "目的地 経度",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSDestBearingRef",
        display: "目的地 方位基準",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSDestBearing",
        display: "目的地 方位",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSDestDistanceRef",
        display: "目的地までの距離単位",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSDestDistance",
        display: "目的地までの距離",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSProcessingMethod",
        display: "GPS 処理方法",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSAreaInformation",
        display: "GPS エリア情報",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSDateStamp",
        display: "GPS 日付",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSDifferential",
        display: "ディファレンシャル補正",
        group: TagGroup::Gps,
    },
    TagInfo {
        name: "GPSHPositioningError",
        display: "水平方向の測位誤差",
        group: TagGroup::Gps,
    },
    // ── その他 / 拡張 ──
    TagInfo {
        name: "ExifVersion",
        display: "EXIF バージョン",
        group: TagGroup::Other,
    },
    TagInfo {
        name: "FlashpixVersion",
        display: "Flashpix バージョン",
        group: TagGroup::Other,
    },
    TagInfo {
        name: "ComponentsConfiguration",
        display: "色成分構成",
        group: TagGroup::Other,
    },
    TagInfo {
        name: "CompressedBitsPerPixel",
        display: "圧縮ビット/ピクセル",
        group: TagGroup::Other,
    },
    TagInfo {
        name: "UserComment",
        display: "ユーザー コメント",
        group: TagGroup::Other,
    },
    TagInfo {
        name: "RelatedSoundFile",
        display: "関連サウンドファイル",
        group: TagGroup::Other,
    },
    TagInfo {
        name: "InteroperabilityIndex",
        display: "相互運用性インデックス",
        group: TagGroup::Other,
    },
    TagInfo {
        name: "InteroperabilityVersion",
        display: "相互運用性バージョン",
        group: TagGroup::Other,
    },
    TagInfo {
        name: "XPTitle",
        display: "タイトル (Windows)",
        group: TagGroup::Other,
    },
    TagInfo {
        name: "XPComment",
        display: "コメント (Windows)",
        group: TagGroup::Other,
    },
    TagInfo {
        name: "XPAuthor",
        display: "作成者 (Windows)",
        group: TagGroup::Other,
    },
    TagInfo {
        name: "XPKeywords",
        display: "キーワード (Windows)",
        group: TagGroup::Other,
    },
    TagInfo {
        name: "XPSubject",
        display: "件名 (Windows)",
        group: TagGroup::Other,
    },
    TagInfo {
        name: "MakerNote",
        display: "メーカーノート",
        group: TagGroup::Other,
    },
    TagInfo {
        name: "PrintImageMatching",
        display: "Print Image Matching",
        group: TagGroup::Other,
    },
    // TIFF Rev 6.0 の構造系タグ。1st IFD (サムネイル) の保管情報で、
    // 一般ユーザーには無価値。default_exif_hidden_tags で既定非表示。
    TagInfo {
        name: "Compression",
        display: "圧縮方式",
        group: TagGroup::Other,
    },
    TagInfo {
        name: "JPEGInterchangeFormat",
        display: "JPEG オフセット",
        group: TagGroup::Other,
    },
    TagInfo {
        name: "JPEGInterchangeFormatLength",
        display: "JPEG データ長",
        group: TagGroup::Other,
    },
];

/// canonical タグ名から日本語表示名を返す。未登録ならそのまま。
pub fn tag_display_name(tag_name: &str) -> &str {
    TAG_REGISTRY
        .iter()
        .find(|t| t.name == tag_name)
        .map(|t| t.display)
        .unwrap_or(tag_name)
}

/// canonical タグ名からグループを返す。未登録は [`TagGroup::Other`]。
pub fn tag_group(tag_name: &str) -> TagGroup {
    TAG_REGISTRY
        .iter()
        .find(|t| t.name == tag_name)
        .map(|t| t.group)
        .unwrap_or(TagGroup::Other)
}

/// 指定グループに属する既知タグの一覧 (登録順)。preferences UI 用。
pub fn known_tags_in_group(group: TagGroup) -> impl Iterator<Item = &'static TagInfo> {
    TAG_REGISTRY.iter().filter(move |t| t.group == group)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_exif_nonexistent_returns_none() {
        assert!(read_exif(Path::new("nonexistent_file.jpg"), &[]).is_none());
    }

    #[test]
    fn read_exif_from_empty_bytes_returns_none() {
        assert!(read_exif_from_bytes(&[], &[]).is_none());
    }

    #[test]
    fn exif_version_is_pretty_formatted() {
        assert_eq!(format_value(36864, "0231"), "2.31");
        assert_eq!(format_value(36864, "0232"), "2.32");
        assert_eq!(format_value(36864, "0100"), "1.00");
        assert_eq!(format_value(40960, "0100"), "1.00");
        assert_eq!(format_value(42035, "0231"), "0231"); // 対象外タグは素通り
    }

    #[test]
    fn orientation_values_are_pretty_formatted() {
        assert_eq!(format_value(274, "Straight"), "標準 (1)");
        assert_eq!(format_value(274, "Unknown (0112=2)"), "左右反転 (2)");
        assert_eq!(format_value(274, "Upside down"), "180度回転 (3)");
        assert_eq!(format_value(274, "Unknown (0112=4)"), "上下反転 (4)");
        assert_eq!(
            format_value(274, "Unknown (0112=5)"),
            "左右反転 + 90度回転 (5)"
        );
        assert_eq!(format_value(274, "Rotated to left"), "90度回転 (6)");
        assert_eq!(
            format_value(274, "Unknown (0112=7)"),
            "左右反転 + 270度回転 (7)"
        );
        assert_eq!(format_value(274, "Rotated to right"), "270度回転 (8)");
    }

    #[test]
    fn unknown_tag_lookup_covers_extensions() {
        use rexif::IfdKind;
        // EXIF 2.31 タイムゾーン
        assert_eq!(name_for_unknown_tag(IfdKind::Exif, 36880), "OffsetTime");
        // Microsoft Windows
        assert_eq!(name_for_unknown_tag(IfdKind::Ifd0, 40092), "XPComment");
        // GPS IFD 拡張
        assert_eq!(
            name_for_unknown_tag(IfdKind::Gps, 31),
            "GPSHPositioningError"
        );
        // Interop IFD は同じ raw でも GPS と区別される
        assert_eq!(
            name_for_unknown_tag(IfdKind::Interoperability, 1),
            "InteroperabilityIndex"
        );
        // 未登録 → Tag(N) フォールバック
        assert_eq!(name_for_unknown_tag(IfdKind::Exif, 65535), "Tag(65535)");
    }
}
