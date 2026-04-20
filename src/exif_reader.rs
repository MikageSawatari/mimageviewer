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

        // GPS IFD は IFD 種別で確実に判定する (タグ ID は他 IFD と衝突するため)。
        // それ以外はタグ ID で大カテゴリに振り分け。
        if e.kind == rexif::IfdKind::Gps {
            gps_fields.push(entry);
            continue;
        }
        match e.ifd.tag {
            // カメラ情報
            271 | 272 | 305 | 42032 | 42033 | 42035 | 42036 | 42037 => {
                camera_fields.push(entry);
            }
            // 撮影設定
            33434 | 33437 | 34850 | 34855 | 34858 | 34864
            | 37377 | 37378 | 37380 | 37381 | 37383 | 37385 | 37386
            | 37888 | 37889 | 37890 | 37891 | 37892 | 37893
            | 41486 | 41487 | 41488 | 41986 | 41987 | 41988 | 41989
            | 41990 | 41991 | 41992 | 41993 | 41994 | 41996
            | 42034 => {
                shooting_fields.push(entry);
            }
            // 画像情報
            256 | 257 | 274 | 306 | 36867 | 36868 | 36880 | 36881 | 36882
            | 37520 | 37521 | 37522
            | 40961 | 40962 | 40963 => {
                image_fields.push(entry);
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
        "HostComputer" => "ホスト コンピュータ",
        "WhitePoint" => "白色点",
        "PrimaryChromaticities" => "原色色度",
        "YCbCrCoefficients" => "YCbCr 係数",
        "YCbCrPositioning" => "YCbCr 配置",
        "ReferenceBlackWhite" => "基準白黒点",
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
    fn unknown_tag_lookup_covers_extensions() {
        use rexif::IfdKind;
        // EXIF 2.31 タイムゾーン
        assert_eq!(name_for_unknown_tag(IfdKind::Exif, 36880), "OffsetTime");
        // Microsoft Windows
        assert_eq!(name_for_unknown_tag(IfdKind::Ifd0, 40092), "XPComment");
        // GPS IFD 拡張
        assert_eq!(name_for_unknown_tag(IfdKind::Gps, 31), "GPSHPositioningError");
        // Interop IFD は同じ raw でも GPS と区別される
        assert_eq!(
            name_for_unknown_tag(IfdKind::Interoperability, 1),
            "InteroperabilityIndex"
        );
        // 未登録 → Tag(N) フォールバック
        assert_eq!(name_for_unknown_tag(IfdKind::Exif, 65535), "Tag(65535)");
    }
}
