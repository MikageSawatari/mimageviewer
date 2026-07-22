//! UI フォントの列挙・取り込み・プレビュー生成。
//!
//! システムフォントの走査、ファイル I/O、ラスタライズは重いため、呼び出し側は
//! 必ずワーカースレッドから実行する。egui の Context / TextureHandle は扱わない。

use ab_glyph::{Font, FontVec, ScaleFont, point};
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};

use crate::settings::{UiFontSelection, UiFontSettings};

const PREVIEW_WIDTH: usize = 640;
const PREVIEW_HEIGHT: usize = 72;
const PREVIEW_FONT_SIZE: f32 = 25.0;
const PREVIEW_TEXT: &str = "mImageViewer  表示サンプル  Aa 0123";
const MAX_UI_FONT_FILE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiFontFace {
    pub selection: UiFontSelection,
    pub label: String,
    pub family: String,
    pub supports_japanese: bool,
    pub supports_latin: bool,
    pub imported: bool,
    pub weight: u16,
    pub variable: bool,
}

impl UiFontFace {
    pub fn search_text(&self) -> String {
        format!("{} {}", self.label, self.family).to_lowercase()
    }
}

pub fn enumerate_ui_fonts() -> Result<Vec<UiFontFace>, String> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let user_dir = crate::font_assets::user_fonts_dir();
    if user_dir.is_dir() {
        db.load_fonts_dir(&user_dir);
    }
    Ok(collect_faces(&db, Some(&user_dir)))
}

pub fn import_ui_font(path: &Path) -> Result<Vec<UiFontFace>, String> {
    if !is_supported_font_path(path) {
        return Err("TrueType/OpenType フォント (.ttf/.otf/.ttc/.otc) を選んでください。".into());
    }
    let size = std::fs::metadata(path)
        .map_err(|err| format!("フォントファイルを確認できませんでした: {err}"))?
        .len();
    if size > MAX_UI_FONT_FILE_BYTES {
        return Err("256 MiB を超えるフォントファイルは追加できません。".into());
    }
    let user_dir = crate::font_assets::user_fonts_dir();
    std::fs::create_dir_all(&user_dir)
        .map_err(|err| format!("フォント保存先を作成できませんでした: {err}"))?;
    let imported_path = copy_font_without_overwrite(path, &user_dir)
        .map_err(|err| format!("フォントを取り込めませんでした: {err}"))?;

    let mut db = fontdb::Database::new();
    if let Err(err) = db.load_font_file(&imported_path) {
        let _ = std::fs::remove_file(&imported_path);
        return Err(format!("フォントを解析できませんでした: {err}"));
    }
    let faces = collect_faces(&db, Some(&user_dir));
    if faces.is_empty() {
        let _ = std::fs::remove_file(&imported_path);
        Err("選択したファイルに日本語の通常書体がありません。".into())
    } else {
        Ok(faces)
    }
}

pub fn render_preview(settings: &UiFontSettings) -> Result<egui::ColorImage, String> {
    let mut normalized = settings.clone();
    normalized.sanitize();
    let (selected, selected_index) = selection_font_bytes(&normalized.selection)
        .or_else(default_font_bytes)
        .ok_or_else(|| "プレビュー用フォントを読み込めませんでした。".to_string())?;
    let selected = FontVec::try_from_vec_and_index(selected, selected_index)
        .map_err(|_| "選択したフォントをラスタライズできませんでした。".to_string())?;
    let fallback = default_font_bytes()
        .and_then(|(data, index)| FontVec::try_from_vec_and_index(data, index).ok());

    let mut alpha = vec![0_u8; PREVIEW_WIDTH * PREVIEW_HEIGHT];
    let selected_scaled = selected.as_scaled(PREVIEW_FONT_SIZE);
    let baseline = PREVIEW_HEIGHT as f32 * 0.5
        + (selected_scaled.ascent() + selected_scaled.descent()) * 0.5
        + normalized.vertical_adjust;
    let mut cursor_x = 14.0_f32;

    for ch in PREVIEW_TEXT.chars() {
        let selected_id = selected.glyph_id(ch);
        let font = if selected_id.0 != 0 {
            &selected
        } else if let Some(fallback) = fallback.as_ref() {
            fallback
        } else {
            &selected
        };
        let scaled = font.as_scaled(PREVIEW_FONT_SIZE);
        let id = font.glyph_id(ch);
        if let Some(outlined) = font
            .outline_glyph(id.with_scale_and_position(PREVIEW_FONT_SIZE, point(cursor_x, baseline)))
        {
            let bounds = outlined.px_bounds();
            outlined.draw(|x, y, coverage| {
                let px = bounds.min.x as i32 + x as i32;
                let py = bounds.min.y as i32 + y as i32;
                if px >= 0
                    && py >= 0
                    && (px as usize) < PREVIEW_WIDTH
                    && (py as usize) < PREVIEW_HEIGHT
                {
                    let value = (coverage.clamp(0.0, 1.0) * 255.0).round() as u8;
                    let slot = &mut alpha[py as usize * PREVIEW_WIDTH + px as usize];
                    *slot = (*slot).max(value);
                }
            });
        }
        cursor_x += scaled.h_advance(id);
        if cursor_x >= PREVIEW_WIDTH as f32 - 14.0 {
            break;
        }
    }

    let pixels = alpha
        .into_iter()
        .map(|a| egui::Color32::from_white_alpha(a))
        .collect();
    Ok(egui::ColorImage::new(
        [PREVIEW_WIDTH, PREVIEW_HEIGHT],
        pixels,
    ))
}

fn collect_faces(db: &fontdb::Database, user_dir: Option<&Path>) -> Vec<UiFontFace> {
    let mut seen = HashSet::new();
    let mut faces = Vec::new();
    for info in db.faces() {
        // UI 本文で Italic / Oblique を常用することは想定しない。Bold 等の weight は
        // 選べるままにし、傾斜 style だけを候補外にする。
        if info.style != fontdb::Style::Normal {
            continue;
        }
        let Some(path) = source_path(&info.source) else {
            continue;
        };
        if !is_supported_font_path(&path) {
            continue;
        }
        let key = format!("{}#{}", path.to_string_lossy().to_lowercase(), info.index);
        if !seen.insert(key) {
            continue;
        }
        let family = info
            .families
            .first()
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| info.post_script_name.clone());
        if family.trim().is_empty() {
            continue;
        }
        let (supports_japanese, supports_latin, upright, variable) = db
            .with_face_data(info.id, |data, index| {
                ttf_parser::Face::parse(data, index)
                    .ok()
                    .map(|face| {
                        (
                            ['今', 'あ']
                                .into_iter()
                                .all(|ch| face.glyph_index(ch).is_some()),
                            ['A', 'a', '0']
                                .into_iter()
                                .all(|ch| face.glyph_index(ch).is_some()),
                            !face.is_italic() && !face.is_oblique(),
                            face.is_variable(),
                        )
                    })
                    .unwrap_or((false, false, false, false))
            })
            .unwrap_or((false, false, false, false));
        // UI 全体で日本語を使うアプリなので、日本語を持たない face は候補外にする。
        // 記号・絵文字などの不足分だけ既定 fallback で補う。
        if !supports_japanese || !upright {
            continue;
        }
        let imported = user_dir
            .and_then(|dir| path.canonicalize().ok().zip(dir.canonicalize().ok()))
            .is_some_and(|(path, dir)| path.starts_with(dir));
        let label = face_label(&family, info.weight, info.style, variable);
        faces.push(UiFontFace {
            selection: UiFontSelection::Face {
                display_name: label.clone(),
                path,
                face_index: info.index,
                post_script_name: info.post_script_name.clone(),
            },
            label,
            family,
            supports_japanese,
            supports_latin,
            imported,
            weight: info.weight.0,
            variable,
        });
    }
    sort_ui_font_faces(&mut faces);
    faces
}

pub fn sort_ui_font_faces(faces: &mut [UiFontFace]) {
    faces.sort_by(|a, b| {
        preferred_family_rank(&a.family)
            .cmp(&preferred_family_rank(&b.family))
            .then_with(|| a.family.to_lowercase().cmp(&b.family.to_lowercase()))
            .then_with(|| a.weight.cmp(&b.weight))
            .then_with(|| a.variable.cmp(&b.variable))
            .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
            .then_with(|| selection_index(&a.selection).cmp(&selection_index(&b.selection)))
    });
}

fn source_path(source: &fontdb::Source) -> Option<PathBuf> {
    match source {
        fontdb::Source::File(path) => Some(path.clone()),
        fontdb::Source::SharedFile(path, _) => Some(path.clone()),
        fontdb::Source::Binary(_) => None,
    }
}

fn face_label(
    family: &str,
    weight: fontdb::Weight,
    style: fontdb::Style,
    variable: bool,
) -> String {
    // OS/2 usWeightClass の標準値をそのまま名前へ対応させる。旧実装は 350..=549 を
    // すべて Regular としたため、Noto Sans JP の Regular (400) / Medium (500) /
    // variable face が同名で並んでいた。非標準値は推測せず数値を表示する。
    let weight = match weight.0 {
        100 => "Thin".to_string(),
        200 => "Extra Light".to_string(),
        300 => "Light".to_string(),
        350 => "Demi Light".to_string(),
        400 => "Regular".to_string(),
        500 => "Medium".to_string(),
        600 => "Semi Bold".to_string(),
        700 => "Bold".to_string(),
        800 => "Extra Bold".to_string(),
        900 => "Black".to_string(),
        value => format!("Weight {value}"),
    };
    let style = match style {
        fontdb::Style::Normal => "",
        fontdb::Style::Italic => " Italic",
        fontdb::Style::Oblique => " Oblique",
    };
    let mut qualifiers = Vec::new();
    if weight != "Regular" {
        qualifiers.push(weight);
    }
    if !style.is_empty() {
        qualifiers.push(style.trim().to_string());
    }
    if variable {
        qualifiers.push("Variable".to_string());
    }
    if qualifiers.is_empty() {
        family.to_owned()
    } else {
        format!("{family} ({})", qualifiers.join(", "))
    }
}

fn preferred_family_rank(family: &str) -> u8 {
    match family.to_ascii_lowercase().as_str() {
        // 日本語 UI で読みやすく、Windows に標準搭載される代表候補を先頭へ置く。
        // フォント名は Windows / font metadata の言語により英語・日本語表記がある。
        "biz udpgothic" | "biz udp gothic" | "biz udpゴシック" => 0,
        "meiryo" | "メイリオ" => 1,
        "meiryo ui" | "メイリオ ui" => 2,
        "yu gothic ui" | "游ゴシック ui" => 3,
        "ms ui gothic" | "ms uiゴシック" => 4,
        _ => 5,
    }
}

fn selection_index(selection: &UiFontSelection) -> u32 {
    match selection {
        UiFontSelection::Face { face_index, .. } => *face_index,
        UiFontSelection::Default | UiFontSelection::Unknown => 0,
    }
}

fn selection_font_bytes(selection: &UiFontSelection) -> Option<(Vec<u8>, u32)> {
    match selection {
        UiFontSelection::Face {
            path, face_index, ..
        } => read_font_file(path).ok().map(|data| (data, *face_index)),
        UiFontSelection::Default | UiFontSelection::Unknown => None,
    }
}

fn default_font_bytes() -> Option<(Vec<u8>, u32)> {
    [
        Path::new(r"C:\Windows\Fonts\YuGothM.ttc"),
        Path::new(r"C:\Windows\Fonts\meiryo.ttc"),
        Path::new(r"C:\Windows\Fonts\msgothic.ttc"),
    ]
    .into_iter()
    .find_map(|path| std::fs::read(path).ok().map(|data| (data, 0)))
}

fn is_supported_font_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "ttf" | "otf" | "ttc" | "otc"
            )
        })
}

fn read_font_file(path: &Path) -> io::Result<Vec<u8>> {
    let metadata = std::fs::metadata(path)?;
    if metadata.len() > MAX_UI_FONT_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "UI font exceeds 256 MiB",
        ));
    }
    std::fs::read(path)
}

fn copy_font_without_overwrite(source: &Path, target_dir: &Path) -> io::Result<PathBuf> {
    // 保存先を create_new する前に入力元を確保する。入力元が消えた / 開けない場合に
    // user_fonts へ空ファイルだけ残さないため、TOCTOU の失敗境界を出力作成より前へ置く。
    let mut input = std::fs::File::open(source)?;
    let stem = source
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("font");
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("ttf")
        .to_ascii_lowercase();
    for suffix in 0..10_000_u32 {
        let name = if suffix == 0 {
            format!("{stem}.{extension}")
        } else {
            format!("{stem}-{suffix}.{extension}")
        };
        let target = target_dir.join(name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
        {
            Ok(mut output) => {
                if let Err(err) = io::copy(&mut input, &mut output) {
                    drop(output);
                    let _ = std::fs::remove_file(&target);
                    return Err(err);
                }
                return Ok(target);
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "同名フォントの保存先を確保できませんでした",
    ))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn system_catalog_preserves_collection_face_indices() {
        let faces = enumerate_ui_fonts().expect("system font enumeration should succeed");
        assert!(!faces.is_empty());
        assert!(faces.iter().all(|face| face.supports_japanese));
        assert!(faces.iter().any(|face| face.supports_latin));
        assert!(
            faces
                .iter()
                .all(|face| { !face.label.contains("Italic") && !face.label.contains("Oblique") })
        );
        let unique = faces
            .iter()
            .map(|face| match &face.selection {
                UiFontSelection::Face {
                    path, face_index, ..
                } => format!("{}#{face_index}", path.display()),
                _ => unreachable!(),
            })
            .collect::<HashSet<_>>();
        assert_eq!(unique.len(), faces.len());
    }

    #[test]
    fn preview_contains_visible_pixels() {
        let image = render_preview(&UiFontSettings::default()).expect("preview should render");
        assert_eq!(image.size, [PREVIEW_WIDTH, PREVIEW_HEIGHT]);
        assert!(image.pixels.iter().any(|pixel| pixel.a() != 0));
    }

    #[test]
    fn copy_font_source_open_failure_does_not_leave_empty_target() {
        let source_dir = tempfile::tempdir().expect("source tempdir");
        let target_dir = tempfile::tempdir().expect("target tempdir");
        let missing_source = source_dir.path().join("missing-font.ttf");

        let err = copy_font_without_overwrite(&missing_source, target_dir.path())
            .expect_err("missing source must fail before creating the target");

        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert_eq!(
            std::fs::read_dir(target_dir.path())
                .expect("target directory")
                .count(),
            0,
            "入力元 open 失敗では保存先に空ファイルを残さない"
        );
    }

    #[test]
    fn recommended_japanese_ui_fonts_are_sorted_first() {
        assert!(preferred_family_rank("BIZ UDPGothic") < preferred_family_rank("Meiryo"));
        assert!(preferred_family_rank("Meiryo") < preferred_family_rank("Meiryo UI"));
        assert!(preferred_family_rank("Meiryo UI") < preferred_family_rank("Arial"));
    }

    #[test]
    fn face_labels_preserve_weight_and_variable_identity() {
        assert_eq!(
            face_label(
                "Noto Sans JP",
                fontdb::Weight::NORMAL,
                fontdb::Style::Normal,
                false,
            ),
            "Noto Sans JP"
        );
        assert_eq!(
            face_label(
                "Noto Sans JP",
                fontdb::Weight::MEDIUM,
                fontdb::Style::Normal,
                false,
            ),
            "Noto Sans JP (Medium)"
        );
        assert_eq!(
            face_label(
                "Noto Sans JP",
                fontdb::Weight::NORMAL,
                fontdb::Style::Normal,
                true,
            ),
            "Noto Sans JP (Variable)"
        );
        assert_eq!(
            face_label(
                "Noto Sans JP",
                fontdb::Weight(350),
                fontdb::Style::Normal,
                false,
            ),
            "Noto Sans JP (Demi Light)"
        );
    }

    #[test]
    fn installed_noto_sans_jp_faces_have_distinct_labels() {
        let paths = [
            r"C:\Windows\Fonts\NotoSansJP-Black.otf",
            r"C:\Windows\Fonts\NotoSansJP-Bold.otf",
            r"C:\Windows\Fonts\NotoSansJP-DemiLight.otf",
            r"C:\Windows\Fonts\NotoSansJP-Light.otf",
            r"C:\Windows\Fonts\NotoSansJP-Medium.otf",
            r"C:\Windows\Fonts\NotoSansJP-Regular.otf",
            r"C:\Windows\Fonts\NotoSansJP-Thin.otf",
            r"C:\Windows\Fonts\NotoSansJP-VF.ttf",
        ];
        let existing = paths
            .into_iter()
            .map(Path::new)
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();
        if existing.len() < 2 {
            return;
        }

        let mut db = fontdb::Database::new();
        for path in existing {
            db.load_font_file(path)
                .unwrap_or_else(|err| panic!("{} should load: {err}", path.display()));
        }
        let faces = collect_faces(&db, None)
            .into_iter()
            .filter(|face| face.family.eq_ignore_ascii_case("Noto Sans JP"))
            .collect::<Vec<_>>();
        let labels = faces
            .iter()
            .map(|face| face.label.as_str())
            .collect::<HashSet<_>>();
        assert_eq!(
            labels.len(),
            faces.len(),
            "Noto Sans JP choices must have unique labels: {:?}",
            faces.iter().map(|face| &face.label).collect::<Vec<_>>()
        );
        assert!(faces.iter().any(|face| face.label.contains("Medium")));
        if Path::new(r"C:\Windows\Fonts\NotoSansJP-VF.ttf").is_file() {
            assert!(faces.iter().any(|face| face.label.contains("Variable")));
        }
    }

    #[test]
    fn catalog_excludes_italic_and_oblique_collection_faces() {
        let path = Path::new(r"C:\Windows\Fonts\meiryo.ttc");
        let mut db = fontdb::Database::new();
        db.load_font_file(path)
            .expect("Meiryo collection should load");
        let faces = collect_faces(&db, None);
        assert!(faces.iter().any(|face| face.family == "Meiryo"));
        assert!(faces.iter().any(|face| face.family == "Meiryo UI"));
        assert!(
            faces
                .iter()
                .all(|face| { !face.label.contains("Italic") && !face.label.contains("Oblique") })
        );
        assert_eq!(faces.len(), 2, "only the two upright Meiryo faces remain");
    }
}
