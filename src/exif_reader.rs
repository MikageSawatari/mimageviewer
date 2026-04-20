//! EXIF メタデータ読み取り。
//!
//! JPEG/TIFF 等の画像ファイルから撮影情報 (カメラ, レンズ, 露出, GPS 等) を抽出する。
//! `rexif` crate を使用（`kamadak-exif` より寛容なパーサー）。

use std::path::Path;

/// 抽出した EXIF 情報
#[derive(Clone, Debug, Default)]
pub struct ExifInfo {
    /// セクションごとにまとめた (セクション名, [(タグ名, 値)]) のリスト
    pub sections: Vec<(String, Vec<(String, String)>)>,
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

    let mut camera_fields = Vec::new();
    let mut shooting_fields = Vec::new();
    let mut image_fields = Vec::new();
    let mut gps_fields = Vec::new();
    let mut other_fields = Vec::new();

    for e in entries {
        let tag_id = e.ifd.tag;

        // 構造タグ (IFD へのオフセット / バイナリ blob) は常に抑止。
        // ユーザーにとって意味がなく、設定での hide 操作の対象にもしない。
        if is_structural_tag(tag_id) {
            continue;
        }

        let tag_name = tag_name_from_id(tag_id);

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

        // タグ固有の整形
        display = format_value(tag_id, &display);

        let entry = (tag_name.clone(), display);

        match tag_id {
            // カメラ情報
            271 | 272 | 42036 | 42035 | 42033 | 42037 | 305 => {
                camera_fields.push(entry);
            }
            // 撮影設定
            33434 | 33437 | 34855 | 34850 | 37380 | 37383 | 37385 | 37386
            | 41989 | 34864 | 37377 | 37378 | 37381 | 34858 => {
                shooting_fields.push(entry);
            }
            // 画像情報
            306 | 36867 | 36868 | 274 | 40961 | 256 | 257 | 40962 | 40963 => {
                image_fields.push(entry);
            }
            // GPS (tag IDs 0-31 in GPS IFD, but rexif uses different numbering)
            _ if tag_name.starts_with("GPS") => {
                gps_fields.push(entry);
            }
            _ => {
                other_fields.push(entry);
            }
        }
    }

    let mut sections = Vec::new();
    if !camera_fields.is_empty() {
        sections.push(("Camera".to_string(), camera_fields));
    }
    if !shooting_fields.is_empty() {
        sections.push(("Shooting".to_string(), shooting_fields));
    }
    if !image_fields.is_empty() {
        sections.push(("Image".to_string(), image_fields));
    }
    if !gps_fields.is_empty() {
        sections.push(("GPS".to_string(), gps_fields));
    }
    if !other_fields.is_empty() {
        sections.push(("Other".to_string(), other_fields));
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

/// 構造的な「IFD オフセット / 内部バイナリ」タグかどうか。
///
/// これらはファイル形式のプラミングで、ユーザー表示上の意味が無い。
/// 値の型や長さで判別できないものもあるので、タグ ID で明示的に抑止する。
fn is_structural_tag(tag_id: u16) -> bool {
    matches!(
        tag_id,
        // 34665 ExifOffset: ExifIFD へのポインタ
        // 34853 GPSInfo: GPS IFD へのポインタ (GPS* タグ自体は別途展開される)
        // 40965 InteropIFDPointer: 相互運用性 IFD へのポインタ
        // 37500 MakerNote: メーカー固有バイナリ (rexif は Blob として出す)
        34665 | 34853 | 40965 | 37500
    )
}

/// EXIF タグ ID からタグ名を返す。
/// `rexif` の `ExifTag` を参照し、既知タグには読みやすい名前を使う。
fn tag_name_from_id(tag_id: u16) -> String {
    match tag_id {
        // 0th IFD
        256 => "ImageWidth".to_string(),
        257 => "ImageLength".to_string(),
        270 => "ImageDescription".to_string(),
        271 => "Make".to_string(),
        272 => "Model".to_string(),
        274 => "Orientation".to_string(),
        282 => "XResolution".to_string(),
        283 => "YResolution".to_string(),
        296 => "ResolutionUnit".to_string(),
        305 => "Software".to_string(),
        306 => "DateTime".to_string(),
        315 => "Artist".to_string(),
        33432 => "Copyright".to_string(),

        // Exif IFD
        33434 => "ExposureTime".to_string(),
        33437 => "FNumber".to_string(),
        34850 => "ExposureProgram".to_string(),
        34852 => "SpectralSensitivity".to_string(),
        34855 => "PhotographicSensitivity".to_string(),
        34856 => "OECF".to_string(),
        34857 => "Interlace".to_string(),
        34858 => "SensitivityType".to_string(),
        34859 => "StandardOutputSensitivity".to_string(),
        34860 => "RecommendedExposureIndex".to_string(),
        34861 => "ISOSpeed".to_string(),
        34862 => "ISOSpeedLatitudeyyy".to_string(),
        34863 => "ISOSpeedLatitudezzz".to_string(),
        34864 => "SensitivityType".to_string(),
        36864 => "ExifVersion".to_string(),
        36867 => "DateTimeOriginal".to_string(),
        36868 => "DateTimeDigitized".to_string(),
        36880 => "OffsetTime".to_string(),
        36881 => "OffsetTimeOriginal".to_string(),
        36882 => "OffsetTimeDigitized".to_string(),
        37121 => "ComponentsConfiguration".to_string(),
        37122 => "CompressedBitsPerPixel".to_string(),
        37377 => "ShutterSpeedValue".to_string(),
        37378 => "ApertureValue".to_string(),
        37379 => "BrightnessValue".to_string(),
        37380 => "ExposureBiasValue".to_string(),
        37381 => "MaxApertureValue".to_string(),
        37382 => "SubjectDistance".to_string(),
        37383 => "MeteringMode".to_string(),
        37384 => "LightSource".to_string(),
        37385 => "Flash".to_string(),
        37386 => "FocalLength".to_string(),
        37396 => "SubjectArea".to_string(),
        37510 => "UserComment".to_string(),
        37520 => "SubSecTime".to_string(),
        37521 => "SubSecTimeOriginal".to_string(),
        37522 => "SubSecTimeDigitized".to_string(),
        37888 => "Temperature".to_string(),
        37889 => "Humidity".to_string(),
        37890 => "Pressure".to_string(),
        37891 => "WaterDepth".to_string(),
        37892 => "Acceleration".to_string(),
        37893 => "CameraElevationAngle".to_string(),
        40091 => "XPTitle".to_string(),
        40092 => "XPComment".to_string(),
        40093 => "XPAuthor".to_string(),
        40094 => "XPKeywords".to_string(),
        40095 => "XPSubject".to_string(),
        40960 => "FlashpixVersion".to_string(),
        40961 => "ColorSpace".to_string(),
        40962 => "PixelXDimension".to_string(),
        40963 => "PixelYDimension".to_string(),
        40964 => "RelatedSoundFile".to_string(),
        41483 => "FlashEnergy".to_string(),
        41484 => "SpatialFrequencyResponse".to_string(),
        41486 => "FocalPlaneXResolution".to_string(),
        41487 => "FocalPlaneYResolution".to_string(),
        41488 => "FocalPlaneResolutionUnit".to_string(),
        41492 => "SubjectLocation".to_string(),
        41493 => "ExposureIndex".to_string(),
        41495 => "SensingMethod".to_string(),
        41728 => "FileSource".to_string(),
        41729 => "SceneType".to_string(),
        41730 => "CFAPattern".to_string(),
        41985 => "CustomRendered".to_string(),
        41986 => "ExposureMode".to_string(),
        41987 => "WhiteBalance".to_string(),
        41988 => "DigitalZoomRatio".to_string(),
        41989 => "FocalLengthIn35mmFilm".to_string(),
        41990 => "SceneCaptureType".to_string(),
        41991 => "GainControl".to_string(),
        41992 => "Contrast".to_string(),
        41993 => "Saturation".to_string(),
        41994 => "Sharpness".to_string(),
        41995 => "DeviceSettingDescription".to_string(),
        41996 => "SubjectDistanceRange".to_string(),
        42016 => "ImageUniqueID".to_string(),
        42032 => "CameraOwnerName".to_string(),
        42033 => "BodySerialNumber".to_string(),
        42034 => "LensSpecification".to_string(),
        42035 => "LensMake".to_string(),
        42036 => "LensModel".to_string(),
        42037 => "LensSerialNumber".to_string(),
        42080 => "CompositeImage".to_string(),
        42240 => "Gamma".to_string(),

        // GPS IFD のタグ (0x0000〜0x001F)。Interop IFD の 1/2
        // (InteroperabilityIndex/Version) と ID が衝突するが、実ファイル上で
        // 観測頻度の高い GPS 側を優先する。Interop IFD は既定で 40965 の
        // ポインタごと is_structural_tag で抑止済み。
        0 => "GPSVersionID".to_string(),
        1 => "GPSLatitudeRef".to_string(),
        2 => "GPSLatitude".to_string(),
        3 => "GPSLongitudeRef".to_string(),
        4 => "GPSLongitude".to_string(),
        5 => "GPSAltitudeRef".to_string(),
        6 => "GPSAltitude".to_string(),
        7 => "GPSTimeStamp".to_string(),
        8 => "GPSSatellites".to_string(),
        9 => "GPSStatus".to_string(),
        10 => "GPSMeasureMode".to_string(),
        11 => "GPSDOP".to_string(),
        12 => "GPSSpeedRef".to_string(),
        13 => "GPSSpeed".to_string(),
        14 => "GPSTrackRef".to_string(),
        15 => "GPSTrack".to_string(),
        16 => "GPSImgDirectionRef".to_string(),
        17 => "GPSImgDirection".to_string(),
        18 => "GPSMapDatum".to_string(),
        19 => "GPSDestLatitudeRef".to_string(),
        20 => "GPSDestLatitude".to_string(),
        21 => "GPSDestLongitudeRef".to_string(),
        22 => "GPSDestLongitude".to_string(),
        23 => "GPSDestBearingRef".to_string(),
        24 => "GPSDestBearing".to_string(),
        25 => "GPSDestDistanceRef".to_string(),
        26 => "GPSDestDistance".to_string(),
        27 => "GPSProcessingMethod".to_string(),
        28 => "GPSAreaInformation".to_string(),
        29 => "GPSDateStamp".to_string(),
        30 => "GPSDifferential".to_string(),
        31 => "GPSHPositioningError".to_string(),

        _ => format!("Tag({})", tag_id),
    }
}

/// EXIF タグ名の日本語表示名を返す。
/// Windows エクスプローラー / NeeView と同等の表記を採用。
pub fn tag_display_name(tag_name: &str) -> &str {
    match tag_name {
        // 0th IFD
        "ImageWidth" => "画像の幅",
        "ImageLength" => "画像の高さ",
        "ImageDescription" => "画像の説明",
        "Make" => "カメラ メーカー",
        "Model" => "カメラ モデル",
        "Orientation" => "向き",
        "XResolution" => "水平方向の解像度",
        "YResolution" => "垂直方向の解像度",
        "ResolutionUnit" => "解像度の単位",
        "Software" => "プログラム名",
        "DateTime" => "変更日時",
        "Artist" => "作成者",
        "Copyright" => "著作権",

        // Exif IFD
        "ExposureTime" => "露出時間",
        "FNumber" => "絞り値",
        "ExposureProgram" => "露出プログラム",
        "PhotographicSensitivity" => "ISO 速度",
        "SensitivityType" => "感度種別",
        "ExposureBiasValue" => "露出補正",
        "DateTimeOriginal" => "撮影日時",
        "DateTimeDigitized" => "取得日時",
        "ShutterSpeedValue" => "シャッタースピード",
        "ApertureValue" => "絞り値 (APEX)",
        "MaxApertureValue" => "最大絞り",
        "MeteringMode" => "測光モード",
        "LightSource" => "光源",
        "Flash" => "フラッシュ モード",
        "FocalLength" => "焦点距離",
        "UserComment" => "ユーザー コメント",
        "FlashpixVersion" => "Flashpix バージョン",
        "ColorSpace" => "色空間",
        "PixelXDimension" => "幅 (pixel)",
        "PixelYDimension" => "高さ (pixel)",
        "FocalPlaneXResolution" => "焦点面 X 解像度",
        "FocalPlaneYResolution" => "焦点面 Y 解像度",
        "FocalPlaneResolutionUnit" => "焦点面解像度の単位",
        "CustomRendered" => "カスタム レンダリング",
        "ExposureMode" => "露出モード",
        "WhiteBalance" => "ホワイト バランス",
        "DigitalZoomRatio" => "デジタル ズーム",
        "FocalLengthIn35mmFilm" => "35mm 焦点距離",
        "SceneCaptureType" => "撮影シーン",
        "BodySerialNumber" => "カメラ製造番号",
        "LensMake" => "レンズ メーカー",
        "LensModel" => "レンズ モデル",
        "LensSerialNumber" => "レンズ製造番号",
        "LensSpecification" => "レンズ スペック",
        "CameraOwnerName" => "カメラ所有者名",

        // Exif 2.3x 拡張
        "ExifVersion" => "EXIF バージョン",
        "FileSource" => "ファイル ソース",
        "SceneType" => "シーン タイプ",
        "CFAPattern" => "CFA パターン",
        "GainControl" => "ゲイン制御",
        "Contrast" => "コントラスト",
        "Saturation" => "彩度",
        "Sharpness" => "シャープネス",
        "SubjectDistanceRange" => "被写体距離レンジ",
        "ImageUniqueID" => "画像固有 ID",
        "BrightnessValue" => "輝度値",
        "SubjectDistance" => "被写体距離",
        "SubjectArea" => "被写体領域",
        "SubSecTime" => "秒以下の時刻",
        "SubSecTimeOriginal" => "秒以下の撮影時刻",
        "SubSecTimeDigitized" => "秒以下のデジタル化時刻",
        "OffsetTime" => "時差",
        "OffsetTimeOriginal" => "撮影時の時差",
        "OffsetTimeDigitized" => "デジタル化時の時差",
        "Temperature" => "温度",
        "Humidity" => "湿度",
        "Pressure" => "気圧",
        "WaterDepth" => "水深",
        "Acceleration" => "加速度",
        "CameraElevationAngle" => "カメラ仰角",
        "XPTitle" => "タイトル (Windows)",
        "XPComment" => "コメント (Windows)",
        "XPAuthor" => "作成者 (Windows)",
        "XPKeywords" => "キーワード (Windows)",
        "XPSubject" => "件名 (Windows)",
        "CompositeImage" => "合成画像",
        "Gamma" => "ガンマ",

        // GPS
        "GPSVersionID" => "GPS バージョン",
        "GPSLatitudeRef" => "緯度基準",
        "GPSLatitude" => "緯度",
        "GPSLongitudeRef" => "経度基準",
        "GPSLongitude" => "経度",
        "GPSAltitudeRef" => "高度基準",
        "GPSAltitude" => "高度",
        "GPSTimeStamp" => "GPS 時刻",
        "GPSSatellites" => "GPS 衛星",
        "GPSStatus" => "GPS 受信状態",
        "GPSMeasureMode" => "GPS 測位モード",
        "GPSDOP" => "GPS 測位精度 (DOP)",
        "GPSSpeedRef" => "速度単位",
        "GPSSpeed" => "移動速度",
        "GPSTrackRef" => "進行方向基準",
        "GPSTrack" => "進行方向",
        "GPSImgDirectionRef" => "撮影方位基準",
        "GPSImgDirection" => "撮影方位",
        "GPSMapDatum" => "測地系",
        "GPSDestLatitudeRef" => "目的地 緯度基準",
        "GPSDestLatitude" => "目的地 緯度",
        "GPSDestLongitudeRef" => "目的地 経度基準",
        "GPSDestLongitude" => "目的地 経度",
        "GPSDestBearingRef" => "目的地 方位基準",
        "GPSDestBearing" => "目的地 方位",
        "GPSDestDistanceRef" => "目的地までの距離単位",
        "GPSDestDistance" => "目的地までの距離",
        "GPSProcessingMethod" => "GPS 処理方法",
        "GPSAreaInformation" => "GPS エリア情報",
        "GPSDateStamp" => "GPS 日付",
        "GPSDifferential" => "ディファレンシャル補正",
        "GPSHPositioningError" => "水平方向の測位誤差",

        // 未知のタグはそのまま返す
        other => other,
    }
}

/// EXIF セクション名の日本語表示名を返す。
pub fn section_display_name(section: &str) -> &str {
    match section {
        "Camera" => "カメラ",
        "Shooting" => "撮影設定",
        "Image" => "画像情報",
        "GPS" => "GPS",
        "Other" => "その他",
        other => other,
    }
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
    fn structural_tags_are_suppressed() {
        assert!(is_structural_tag(34665));
        assert!(is_structural_tag(34853));
        assert!(is_structural_tag(40965));
        assert!(is_structural_tag(37500));
        assert!(!is_structural_tag(271)); // Make は通常タグ
    }
}
