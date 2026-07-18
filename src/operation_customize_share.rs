//! 操作カスタマイズ 3 点セットの共有形式と差分計算。
//!
//! ファイル I/O や App 状態に依存せず、JSON 文字列と `Settings` の間の変換、
//! 取り込み値の正規化、実効キー割り当ての差分だけを扱う。

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::keymap::{
    KeyAction, Keymap, KeymapSettings, MenuCommandId, MenuCommandOrderSettings, MenuLayoutSettings,
    TopMenuId, menu_command_can_be_hidden, menu_command_spec,
};
use crate::ring_shortcut::RingShortcutSettings;
use crate::settings::Settings;

pub const OPERATION_CUSTOMIZE_FORMAT: &str = "mimageviewer.operation-customize";
pub const OPERATION_CUSTOMIZE_FORMAT_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationCustomizeBundle {
    pub format: String,
    pub format_version: u32,
    #[serde(default)]
    pub app_version: String,
    #[serde(default)]
    pub exported_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub keymap: KeymapSettings,
    #[serde(default)]
    pub ring_shortcuts: RingShortcutSettings,
    #[serde(default)]
    pub menu_layout: MenuLayoutSettings,
}

impl OperationCustomizeBundle {
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            format: OPERATION_CUSTOMIZE_FORMAT.to_string(),
            format_version: OPERATION_CUSTOMIZE_FORMAT_VERSION,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            exported_at: utc_now_iso8601(),
            label: None,
            keymap: settings.keymap.clone(),
            ring_shortcuts: settings.ring_shortcuts.clone(),
            menu_layout: settings.menu_layout.clone(),
        }
    }

    pub fn defaults() -> Self {
        Self {
            format: OPERATION_CUSTOMIZE_FORMAT.to_string(),
            format_version: OPERATION_CUSTOMIZE_FORMAT_VERSION,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            exported_at: utc_now_iso8601(),
            label: Some("??".to_string()),
            keymap: KeymapSettings::default(),
            ring_shortcuts: RingShortcutSettings::default(),
            menu_layout: MenuLayoutSettings::default(),
        }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        let label = label.into().trim().to_string();
        self.label = (!label.is_empty()).then_some(label);
        self
    }

    pub fn apply_to(&self, settings: &mut Settings) {
        settings.keymap = self.keymap.clone();
        settings.ring_shortcuts = self.ring_shortcuts.clone();
        settings.menu_layout = self.menu_layout.clone();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedImport {
    pub bundle: OperationCustomizeBundle,
    pub warnings: Vec<String>,
    pub ignored_items: usize,
}

#[derive(Debug)]
pub enum ImportError {
    Json(serde_json::Error),
    WrongFormat(String),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => write!(f, "JSON を読み取れませんでした: {error}"),
            Self::WrongFormat(format) => {
                write!(f, "操作カスタマイズ用のファイルではありません: {format}")
            }
        }
    }
}

impl std::error::Error for ImportError {}

impl From<serde_json::Error> for ImportError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

pub fn to_json(bundle: &OperationCustomizeBundle) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(bundle).map(|mut json| {
        json.push('\n');
        json
    })
}

pub fn parse_json(json: &str) -> Result<ParsedImport, ImportError> {
    let mut bundle: OperationCustomizeBundle = serde_json::from_str(json)?;
    if bundle.format != OPERATION_CUSTOMIZE_FORMAT {
        return Err(ImportError::WrongFormat(bundle.format));
    }

    let mut warnings = Vec::new();
    let mut ignored_items = 0;
    if bundle.format_version != OPERATION_CUSTOMIZE_FORMAT_VERSION {
        warnings.push(format!(
            "ファイル形式の版が異なります (ファイル: {}, 対応: {})。読み取れる項目だけ取り込みます。",
            bundle.format_version, OPERATION_CUSTOMIZE_FORMAT_VERSION
        ));
    }
    if !bundle.app_version.is_empty() && bundle.app_version != env!("CARGO_PKG_VERSION") {
        warnings.push(format!(
            "別のアプリ版で作成されたファイルです ({} -> {})。",
            bundle.app_version,
            env!("CARGO_PKG_VERSION")
        ));
    }

    let parsed_keymap = Keymap::from_settings(&bundle.keymap);
    warnings.extend(parsed_keymap.warnings().iter().cloned());
    let original_override_count = bundle.keymap.overrides.len();
    let canonical_keymap = KeymapSettings::from_keymap(&parsed_keymap);
    let mut ordered_overrides = Vec::with_capacity(canonical_keymap.overrides.len());
    for (index, input) in bundle.keymap.overrides.iter().enumerate() {
        let Some(action) = KeyAction::parse_ini_name(&input.action) else {
            continue;
        };
        let is_last_for_action = !bundle.keymap.overrides[index + 1..]
            .iter()
            .any(|later| KeyAction::parse_ini_name(&later.action) == Some(action));
        if !is_last_for_action {
            continue;
        }
        if let Some(canonical) = canonical_keymap
            .overrides
            .iter()
            .find(|candidate| candidate.action == action.ini_name())
        {
            ordered_overrides.push(canonical.clone());
        }
    }
    ignored_items += original_override_count.saturating_sub(ordered_overrides.len());
    bundle.keymap.overrides = ordered_overrides;

    let original_ring = bundle.ring_shortcuts.clone();
    bundle.ring_shortcuts.sanitize();
    if original_ring != bundle.ring_shortcuts {
        let changed = structured_change_count(&original_ring, &bundle.ring_shortcuts).max(1);
        ignored_items += changed;
        warnings.push(format!(
            "対応していないリング/マウス設定を {changed} 件、標準値へ置き換えました。"
        ));
    }

    let (menu_layout, menu_warnings, ignored_menu_items) =
        sanitize_menu_layout(&bundle.menu_layout);
    bundle.menu_layout = menu_layout;
    warnings.extend(menu_warnings);
    ignored_items += ignored_menu_items;

    Ok(ParsedImport {
        bundle,
        warnings,
        ignored_items,
    })
}

fn sanitize_menu_layout(input: &MenuLayoutSettings) -> (MenuLayoutSettings, Vec<String>, usize) {
    let mut warnings = Vec::new();
    let mut ignored = 0;

    let mut seen_top = HashSet::new();
    let top_menu_order = input
        .top_menu_order
        .iter()
        .filter_map(|name| match TopMenuId::parse_stable_name(name) {
            Some(id) if seen_top.insert(id) => Some(id.stable_name().to_string()),
            Some(_) => {
                ignored += 1;
                warnings.push(format!("重複したメニュー項目 '{name}' を無視しました。"));
                None
            }
            None => {
                ignored += 1;
                warnings.push(format!("未知のメニュー '{name}' を無視しました。"));
                None
            }
        })
        .collect();

    let mut seen_parents = HashSet::new();
    let mut command_order = Vec::new();
    for group in &input.command_order {
        let Some(parent) = TopMenuId::parse_stable_name(&group.parent) else {
            ignored += 1;
            warnings.push(format!(
                "未知のメニュー '{}' を無視しました。",
                group.parent
            ));
            continue;
        };
        if !seen_parents.insert(parent) {
            ignored += 1;
            warnings.push(format!(
                "重複したメニュー順序 '{}' を無視しました。",
                group.parent
            ));
            continue;
        }
        let mut seen_commands = HashSet::new();
        let commands = group
            .commands
            .iter()
            .filter_map(|name| {
                let Some(id) = MenuCommandId::parse_stable_name(name) else {
                    ignored += 1;
                    warnings.push(format!("未知のメニュー操作 '{name}' を無視しました。"));
                    return None;
                };
                if menu_command_spec(id).is_none_or(|spec| spec.parent != parent) {
                    ignored += 1;
                    warnings.push(format!(
                        "別のメニューに属する操作 '{name}' を無視しました。"
                    ));
                    return None;
                }
                if !seen_commands.insert(id) {
                    ignored += 1;
                    warnings.push(format!("重複したメニュー項目 '{name}' を無視しました。"));
                    return None;
                }
                Some(id.stable_name().to_string())
            })
            .collect();
        command_order.push(MenuCommandOrderSettings {
            parent: parent.stable_name().to_string(),
            commands,
        });
    }

    let mut seen_hidden = HashSet::new();
    let hidden_commands = input
        .hidden_commands
        .iter()
        .filter_map(|name| {
            let Some(id) = MenuCommandId::parse_stable_name(name) else {
                ignored += 1;
                warnings.push(format!("未知の非表示操作 '{name}' を無視しました。"));
                return None;
            };
            if !menu_command_can_be_hidden(id) {
                ignored += 1;
                warnings.push(format!("非表示にできない操作 '{name}' を無視しました。"));
                return None;
            }
            if !seen_hidden.insert(id) {
                ignored += 1;
                warnings.push(format!("重複した非表示操作 '{name}' を無視しました。"));
                return None;
            }
            Some(id.stable_name().to_string())
        })
        .collect();

    (
        MenuLayoutSettings {
            top_menu_order,
            command_order,
            hidden_commands,
        },
        warnings,
        ignored,
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyDiffKind {
    Added,
    Removed,
    Changed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyDiff {
    pub action: KeyAction,
    pub kind: KeyDiffKind,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationDiff {
    pub key_changes: Vec<KeyDiff>,
    pub ring_change_count: usize,
    pub menu_change_count: usize,
}

impl OperationDiff {
    pub fn is_empty(&self) -> bool {
        self.key_changes.is_empty() && self.ring_change_count == 0 && self.menu_change_count == 0
    }
}

pub fn diff(a: &OperationCustomizeBundle, b: &OperationCustomizeBundle) -> OperationDiff {
    let a_keymap = Keymap::from_settings(&a.keymap);
    let b_keymap = Keymap::from_settings(&b.keymap);
    let key_changes = KeyAction::all()
        .iter()
        .copied()
        .filter_map(|action| {
            let before_chords = a_keymap.effective_chords(action);
            let after_chords = b_keymap.effective_chords(action);
            if before_chords == after_chords {
                return None;
            }
            let kind = match (before_chords.is_empty(), after_chords.is_empty()) {
                (true, false) => KeyDiffKind::Added,
                (false, true) => KeyDiffKind::Removed,
                _ => KeyDiffKind::Changed,
            };
            Some(KeyDiff {
                action,
                kind,
                before: before_chords
                    .into_iter()
                    .map(|chord| chord.display_name())
                    .collect(),
                after: after_chords
                    .into_iter()
                    .map(|chord| chord.display_name())
                    .collect(),
            })
        })
        .collect();
    OperationDiff {
        key_changes,
        ring_change_count: structured_change_count(&a.ring_shortcuts, &b.ring_shortcuts),
        menu_change_count: structured_change_count(&a.menu_layout, &b.menu_layout),
    }
}

fn structured_change_count<T: Serialize>(a: &T, b: &T) -> usize {
    let Ok(a) = serde_json::to_value(a) else {
        return 0;
    };
    let Ok(b) = serde_json::to_value(b) else {
        return 0;
    };
    value_change_count(&a, &b)
}

fn value_change_count(a: &serde_json::Value, b: &serde_json::Value) -> usize {
    if a == b {
        return 0;
    }
    match (a, b) {
        (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
            let keys: HashSet<_> = a.keys().chain(b.keys()).collect();
            keys.into_iter()
                .map(|key| match (a.get(key), b.get(key)) {
                    (Some(a), Some(b)) => value_change_count(a, b),
                    _ => 1,
                })
                .sum()
        }
        (serde_json::Value::Array(a), serde_json::Value::Array(b)) => {
            let shared = a.len().min(b.len());
            (0..shared)
                .map(|index| value_change_count(&a[index], &b[index]))
                .sum::<usize>()
                + a.len().abs_diff(b.len())
        }
        _ => 1,
    }
}

fn utc_now_iso8601() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let (year, month, day, hour, minute, second) = unix_to_ymdhms(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn unix_to_ymdhms(mut secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let second = (secs % 60) as u32;
    secs /= 60;
    let minute = (secs % 60) as u32;
    secs /= 60;
    let hour = (secs % 24) as u32;
    let mut days = secs / 24;
    let mut year = 1970;
    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }
    let month_days = [
        31,
        if is_leap_year(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1;
    let mut day = days as u32 + 1;
    for days_in_month in month_days {
        if day <= days_in_month {
            break;
        }
        day -= days_in_month;
        month += 1;
    }
    (year, month, day, hour, minute, second)
}

fn is_leap_year(year: u32) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::{KeyBindingOverride, MenuCommandId};
    use crate::ring_shortcut::RingActionId;

    fn binding(action: &str, chords: &[&str]) -> KeyBindingOverride {
        KeyBindingOverride {
            action: action.to_string(),
            chords: chords.iter().map(|chord| (*chord).to_string()).collect(),
        }
    }

    #[test]
    fn bundle_json_roundtrip_preserves_three_field_set_and_ignores_future_fields() {
        let mut settings = Settings::default();
        settings.keymap.legacy_ini_migration_done = true;
        settings.keymap.overrides = vec![
            binding("FsSlideshow", &["W"]),
            binding("GridPin", &["Ctrl+P"]),
        ];
        settings.menu_layout.hidden_commands = vec![
            MenuCommandId::SettingsOperationCustomize
                .stable_name()
                .to_string(),
        ];
        settings.ring_shortcuts.grid.slots[0] = RingActionId::CloseMainWindow;
        settings.ring_shortcuts.grid.slots[1] = RingActionId::GridScrollBottom;
        settings.ring_shortcuts.mouse_buttons_grid.middle = RingActionId::QuitApplication;
        let original = OperationCustomizeBundle::from_settings(&settings).with_label("共有用");
        let mut json: serde_json::Value =
            serde_json::from_str(&to_json(&original).unwrap()).unwrap();
        json["future_field"] = serde_json::json!({"anything": true});
        let parsed = parse_json(&serde_json::to_string(&json).unwrap()).unwrap();

        assert_eq!(parsed.bundle, original);
        assert_eq!(
            parsed.bundle.ring_shortcuts.grid.slots[0],
            RingActionId::CloseMainWindow
        );
        assert_eq!(
            parsed.bundle.ring_shortcuts.grid.slots[1],
            RingActionId::GridScrollBottom
        );
        assert_eq!(
            parsed.bundle.ring_shortcuts.mouse_buttons_grid.middle,
            RingActionId::QuitApplication
        );
        assert_eq!(parsed.ignored_items, 0);
    }

    #[test]
    fn import_warns_and_skips_unknown_actions_and_invalid_chords() {
        let mut bundle = OperationCustomizeBundle::defaults();
        bundle.keymap.overrides = vec![
            binding("GridPin", &["Ctrl+P"]),
            binding("FutureAction", &["F24"]),
            binding("FsSlideshow", &["DefinitelyNotAKey"]),
        ];
        let parsed = parse_json(&to_json(&bundle).unwrap()).unwrap();
        assert_eq!(parsed.bundle.keymap.overrides.len(), 1);
        assert_eq!(parsed.bundle.keymap.overrides[0].action, "GridPin");
        assert_eq!(parsed.ignored_items, 2);
        assert!(
            parsed
                .warnings
                .iter()
                .any(|warning| warning.contains("FutureAction"))
        );
        assert!(
            parsed
                .warnings
                .iter()
                .any(|warning| warning.contains("DefinitelyNotAKey"))
        );
    }

    #[test]
    fn effective_chord_diff_classifies_added_removed_changed_and_order_changes() {
        let defaults = OperationCustomizeBundle::defaults();
        let mut current = defaults.clone();
        current.keymap.overrides = vec![
            binding("GridToggleStackMode", &["Q"]),
            binding("FsSlideshow", &[]),
            binding("GridPin", &["Ctrl+P"]),
            binding("FsToggleMetadata", &["Tab", "I"]),
        ];
        let result = diff(&defaults, &current);
        for (action, kind) in [
            (KeyAction::GridToggleStackMode, KeyDiffKind::Added),
            (KeyAction::FsSlideshow, KeyDiffKind::Removed),
            (KeyAction::GridPin, KeyDiffKind::Changed),
        ] {
            assert_eq!(
                result
                    .key_changes
                    .iter()
                    .find(|change| change.action == action)
                    .unwrap()
                    .kind,
                kind
            );
        }
        assert!(
            result
                .key_changes
                .iter()
                .any(|change| change.action == KeyAction::FsToggleMetadata),
            "multiple chord order is significant"
        );
    }

    #[test]
    fn apply_replaces_only_operation_customization_fields() {
        let mut settings = Settings::default();
        settings.add_favorite("keep".to_string(), std::path::PathBuf::from(r"C:\keep"));
        settings.keymap.overrides = vec![binding("GridPin", &["Q"])];
        let mut imported = OperationCustomizeBundle::defaults();
        imported.keymap.overrides = vec![binding("FsSlideshow", &["W"])];
        imported.apply_to(&mut settings);
        assert_eq!(settings.keymap.overrides, imported.keymap.overrides);
        assert_eq!(settings.favorites.len(), 1);
    }

    #[test]
    fn applying_defaults_resets_all_operation_customization_fields() {
        let mut settings = Settings::default();
        settings.keymap.overrides = vec![binding("GridPin", &["Q"])];
        settings.ring_shortcuts.mouse_ring_help_visible =
            !RingShortcutSettings::default().mouse_ring_help_visible;
        settings.menu_layout.hidden_commands = vec![
            MenuCommandId::SettingsOperationCustomize
                .stable_name()
                .to_string(),
        ];

        OperationCustomizeBundle::defaults().apply_to(&mut settings);

        assert_eq!(settings.keymap, KeymapSettings::default());
        assert_eq!(settings.ring_shortcuts, RingShortcutSettings::default());
        assert_eq!(settings.menu_layout, MenuLayoutSettings::default());
    }
}
