//! Pure same-name folder/book policy and the released preference decoder.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{collections::HashSet, path::Path};

pub const KEY: &str = "same_name_folder_book";
pub const LEGACY_KEY: &str = "skip_zip_if_folder_exists";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SameNameFolderBookPolicy {
    #[default]
    PreferBook,
    PreferFolder,
    ShowBoth,
}

/// The new key wins, including when malformed: never replace a future value
/// with the legacy bool. Missing keys remain missing for partial imports.
pub fn canonicalize(map: &mut Map<String, Value>) -> Result<(), String> {
    if let Some(value) = map.get(KEY) {
        serde_json::from_value::<SameNameFolderBookPolicy>(value.clone())
            .map_err(|error| format!("unknown variant for {KEY}: {error}"))?;
    } else if let Some(value) = map.get(LEGACY_KEY) {
        let old = value
            .as_bool()
            .ok_or_else(|| format!("invalid bool for {LEGACY_KEY}"))?;
        let policy = if old {
            SameNameFolderBookPolicy::PreferBook
        } else {
            SameNameFolderBookPolicy::ShowBoth
        };
        map.insert(
            KEY.into(),
            serde_json::to_value(policy).expect("policy is serializable"),
        );
    }
    map.remove(LEGACY_KEY);
    Ok(())
}

/// Callers supply actual candidate kinds after their existing eligibility
/// checks; extension-shaped directories therefore remain folders.
pub fn keep_mask<'a>(
    policy: SameNameFolderBookPolicy,
    candidates: impl IntoIterator<Item = (&'a Path, bool)>,
) -> Vec<bool> {
    let rows: Vec<_> = candidates
        .into_iter()
        .map(|(path, folder)| {
            let name = if folder {
                path.file_name()
            } else {
                path.file_stem()
            };
            (
                name.and_then(|name| name.to_str()).map(str::to_lowercase),
                folder,
            )
        })
        .collect();
    let folders: HashSet<_> = rows
        .iter()
        .filter(|(_, folder)| *folder)
        .filter_map(|(name, _)| name.as_ref())
        .collect();
    let books: HashSet<_> = rows
        .iter()
        .filter(|(_, folder)| !*folder)
        .filter_map(|(name, _)| name.as_ref())
        .collect();
    rows.iter()
        .map(|(name, folder)| match (policy, name) {
            (SameNameFolderBookPolicy::PreferBook, Some(name)) if *folder => !books.contains(name),
            (SameNameFolderBookPolicy::PreferFolder, Some(name)) if !folder => {
                !folders.contains(name)
            }
            _ => true,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn released_bool_and_new_precedence() {
        for (old, expected) in [(true, "prefer_book"), (false, "show_both")] {
            let mut map = serde_json::json!({LEGACY_KEY: old})
                .as_object()
                .unwrap()
                .clone();
            canonicalize(&mut map).unwrap();
            assert_eq!(map[KEY], expected);
            assert!(!map.contains_key(LEGACY_KEY));
        }
        let mut map = serde_json::json!({KEY:"prefer_folder", LEGACY_KEY:true})
            .as_object()
            .unwrap()
            .clone();
        canonicalize(&mut map).unwrap();
        assert_eq!(map[KEY], "prefer_folder");
        let mut map = serde_json::json!({KEY:"future", LEGACY_KEY:false})
            .as_object()
            .unwrap()
            .clone();
        assert!(canonicalize(&mut map).is_err());
    }
    #[test]
    fn same_name_folder_book_legacy_settings_json_preserves_unrelated_values() {
        use crate::settings::Settings;
        for (old, expected) in [
            (true, SameNameFolderBookPolicy::PreferBook),
            (false, SameNameFolderBookPolicy::ShowBoth),
        ] {
            let mut map = serde_json::to_value(Settings::default())
                .unwrap()
                .as_object()
                .unwrap()
                .clone();
            map.remove(KEY);
            map.insert(LEGACY_KEY.into(), Value::Bool(old));
            map.insert("grid_cols".into(), Value::from(9));
            let loaded: Settings = serde_json::from_value(Value::Object(map)).unwrap();
            assert_eq!(loaded.same_name_folder_book, expected);
            assert_eq!(loaded.grid_cols, 9);
            let saved = serde_json::to_value(loaded).unwrap();
            assert!(!saved.as_object().unwrap().contains_key(LEGACY_KEY));
        }
    }
    #[test]
    fn same_name_folder_book_radio_input_retains_all_three_choices() {
        use egui_kittest::kittest::Queryable;
        let mut harness = crate::test_ui::builder()
            .with_size(egui::vec2(260.0, 120.0))
            .build_ui_state(
                |ctx, policy| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        crate::ui_dialogs::preferences::draw_same_name_folder_book_settings(
                            ui, policy,
                        );
                    });
                },
                SameNameFolderBookPolicy::default(),
            );
        harness.run();
        for (label, expected) in [
            ("フォルダを表示", SameNameFolderBookPolicy::PreferFolder),
            ("両方表示", SameNameFolderBookPolicy::ShowBoth),
            ("本を表示", SameNameFolderBookPolicy::PreferBook),
        ] {
            harness.get_by_label(label).click();
            harness.run();
            assert_eq!(*harness.state(), expected);
        }
    }
    #[test]
    fn actual_kinds_and_all_three_choices() {
        let rows = [
            (Path::new("P/Book.zip"), true),
            (Path::new("P/Book.zip.pdf"), false),
            (Path::new("P/Other"), true),
        ];
        assert_eq!(
            keep_mask(SameNameFolderBookPolicy::PreferBook, rows),
            [false, true, true]
        );
        assert_eq!(
            keep_mask(SameNameFolderBookPolicy::PreferFolder, rows),
            [true, false, true]
        );
        assert_eq!(
            keep_mask(SameNameFolderBookPolicy::ShowBoth, rows),
            [true, true, true]
        );
    }
}
