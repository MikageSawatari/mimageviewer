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
        let display = if value.starts_with("[tag=") {
            if let Some(end) = value.find("] ") {
                value[end + 2..].to_string()
            } else {
                value.to_string()
            }
        } else {
            value.to_string()
        };

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
        34855 => "PhotographicSensitivity".to_string(),
        34858 => "SensitivityType".to_string(),
        34864 => "ExposureBiasValue".to_string(),
        36867 => "DateTimeOriginal".to_string(),
        36868 => "DateTimeDigitized".to_string(),
        37377 => "ShutterSpeedValue".to_string(),
        37378 => "ApertureValue".to_string(),
        37380 => "ExposureBiasValue".to_string(),
        37381 => "MaxApertureValue".to_string(),
        37383 => "MeteringMode".to_string(),
        37384 => "LightSource".to_string(),
        37385 => "Flash".to_string(),
        37386 => "FocalLength".to_string(),
        37510 => "UserComment".to_string(),
        40960 => "FlashpixVersion".to_string(),
        40961 => "ColorSpace".to_string(),
        40962 => "PixelXDimension".to_string(),
        40963 => "PixelYDimension".to_string(),
        41486 => "FocalPlaneXResolution".to_string(),
        41487 => "FocalPlaneYResolution".to_string(),
        41488 => "FocalPlaneResolutionUnit".to_string(),
        41985 => "CustomRendered".to_string(),
        41986 => "ExposureMode".to_string(),
        41987 => "WhiteBalance".to_string(),
        41988 => "DigitalZoomRatio".to_string(),
        41989 => "FocalLengthIn35mmFilm".to_string(),
        41990 => "SceneCaptureType".to_string(),
        42033 => "BodySerialNumber".to_string(),
        42035 => "LensMake".to_string(),
        42036 => "LensModel".to_string(),
        42037 => "LensSerialNumber".to_string(),

        // GPS
        0 => "GPSVersionID".to_string(),
        1 => "GPSLatitudeRef".to_string(),
        2 => "GPSLatitude".to_string(),
        3 => "GPSLongitudeRef".to_string(),
        4 => "GPSLongitude".to_string(),
        5 => "GPSAltitudeRef".to_string(),
        6 => "GPSAltitude".to_string(),
        7 => "GPSTimeStamp".to_string(),
        29 => "GPSDateStamp".to_string(),

        _ => format!("Tag({})", tag_id),
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
}
