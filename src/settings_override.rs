//! Apply a JSON overlay to the loaded settings, for scripted runs.
//!
//! Reproducing a display bug usually means putting the viewer in a particular configuration first:
//! the livelock this was written for needs AI upscaling, colorize and continuous reading all on,
//! and a fresh profile has none of them. Driving the preferences dialog from a script to get there
//! would test the dialog rather than the thing under investigation, and would have to be rewritten
//! every time the dialog moves.
//!
//! `Settings` already round-trips through serde, so an overlay costs nothing per field and reaches
//! every setting there is - including ones added later. The overlay is merged into the serialized
//! form and read back, so a partial object changes only what it names:
//!
//! ```json
//! { "ai_feature_mode": "high_quality", "global_preset": { "colorize": { "mode": "monochrome_only" } } }
//! ```
//!
//! Names and values are the *serialized* forms, not what the preferences dialog shows: most enums
//! are snake_case. A name that does not exist stops the run rather than being dropped, because
//! `Settings` ignores unknown fields when it loads - it has to, so that older profiles still open -
//! and a silently ignored typo would leave the scenario running in a configuration nobody chose.
//!
//! Two things keep this away from real profiles. It is behind the `test-script` feature, so it is
//! not in a shipping binary at all; and it refuses to run without an explicit `--data-dir`, so even
//! a developer's test build cannot rewrite the settings they actually use.

use std::ffi::OsString;

use crate::settings::Settings;

const FLAG: &str = "--settings-override";

/// What went wrong, in terms of what the caller can do about it.
#[derive(Debug)]
pub(crate) enum OverrideError {
    NoDataDir,
    Unreadable {
        path: String,
        source: std::io::Error,
    },
    NotJson(serde_json::Error),
    NotAnObject,
    UnknownSetting {
        path: String,
    },
    Rejected(serde_json::Error),
}

impl std::fmt::Display for OverrideError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoDataDir => write!(
                f,
                "{FLAG} requires an explicit --data-dir, so it cannot rewrite the settings of a \
                 profile someone is using"
            ),
            Self::Unreadable { path, source } => {
                write!(f, "{FLAG}: cannot read {path}: {source}")
            }
            Self::NotJson(source) => write!(f, "{FLAG}: not valid JSON: {source}"),
            Self::NotAnObject => write!(
                f,
                "{FLAG}: expected a JSON object of settings to change, e.g. \
                 {{\"ai_feature_mode\":\"high_quality\"}}"
            ),
            Self::UnknownSetting { path } => write!(
                f,
                "{FLAG}: no such setting: {path}. Names are the serialized field names, not the                  labels in the dialog, and values are the serialized forms too (snake_case for                  most enums)"
            ),
            Self::Rejected(source) => write!(
                f,
                "{FLAG}: the result is not valid settings: {source}. A name or value is probably \
                 wrong - they are the serialized field names, not the labels in the dialog"
            ),
        }
    }
}

/// The overlay requested on the command line, if any.
///
/// A value starting with `{` is the JSON itself; anything else is a path to a file containing it.
/// Inline keeps a one-line scenario on one line; a file keeps a long one readable.
pub(crate) fn requested(args: &[OsString]) -> Option<OsString> {
    args.windows(2)
        .find(|window| window[0] == FLAG)
        .map(|window| window[1].clone())
}

/// Merge the overlay into `settings`, reporting what it changed.
///
/// `has_explicit_data_dir` is the caller's answer rather than something read here, because whether
/// the run is isolated is a property of the whole invocation and is already known by then.
pub(crate) fn apply(
    settings: &mut Settings,
    request: &OsString,
    has_explicit_data_dir: bool,
) -> Result<Vec<String>, OverrideError> {
    if !has_explicit_data_dir {
        return Err(OverrideError::NoDataDir);
    }
    let raw = request.to_string_lossy();
    let text = if raw.trim_start().starts_with('{') {
        raw.into_owned()
    } else {
        std::fs::read_to_string(raw.as_ref()).map_err(|source| OverrideError::Unreadable {
            path: raw.into_owned(),
            source,
        })?
    };
    // A BOM is not JSON, and every Windows way of producing a text file adds one: PowerShell
    // 5.1's UTF8 encoding writes it, and so does Notepad. Rejecting a file over three invisible
    // bytes would be an error nobody could act on.
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let overlay: serde_json::Value = serde_json::from_str(text).map_err(OverrideError::NotJson)?;
    if !overlay.is_object() {
        return Err(OverrideError::NotAnObject);
    }

    let mut current = serde_json::to_value(&*settings).map_err(OverrideError::Rejected)?;
    let mut changed = Vec::new();
    merge(&mut current, &overlay, "", &mut changed)?;
    *settings = serde_json::from_value(current).map_err(OverrideError::Rejected)?;
    Ok(changed)
}

/// Recursive merge: objects are combined field by field, everything else is replaced.
///
/// Recursing rather than replacing is what lets an overlay name one nested field without restating
/// its siblings - `global_preset.colorize.mode` alone, leaving the rest of the preset as loaded.
/// Arrays are replaced whole: there is no meaningful way to merge two lists positionally, and
/// half-merging one would be a surprise no error could explain.
fn merge(
    target: &mut serde_json::Value,
    overlay: &serde_json::Value,
    path: &str,
    changed: &mut Vec<String>,
) -> Result<(), OverrideError> {
    let (Some(target_map), Some(overlay_map)) = (target.as_object_mut(), overlay.as_object())
    else {
        if target != overlay {
            changed.push(format!("{path} = {overlay}"));
            *target = overlay.clone();
        }
        return Ok(());
    };
    for (key, value) in overlay_map {
        let child_path = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        // A name the settings do not have stops the run. `Settings` ignores unknown fields when it
        // deserializes - it has to, for old profiles to keep loading - so a typo would otherwise
        // be dropped without a word and the scenario would run in a configuration nobody asked
        // for. Checking against the serialized settings here is the only place it is visible.
        let Some(existing) = target_map.get_mut(key) else {
            return Err(OverrideError::UnknownSetting { path: child_path });
        };
        merge(existing, value, &child_path, changed)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlay(json: &str) -> OsString {
        OsString::from(json)
    }

    #[test]
    fn a_nested_field_changes_without_restating_its_siblings() {
        let mut settings = Settings::default();
        settings.global_preset.brightness = 0.25;
        let changed = apply(
            &mut settings,
            &overlay(r#"{"global_preset":{"colorize":{"mode":"monochrome_only"}}}"#),
            true,
        )
        .expect("overlay should apply");
        assert_eq!(
            settings.global_preset.colorize.mode,
            crate::colorize::ColorizeMode::MonochromeOnly
        );
        assert!(
            (settings.global_preset.brightness - 0.25).abs() < f32::EPSILON,
            "a sibling of the named field must survive the merge"
        );
        assert_eq!(
            changed,
            vec!["global_preset.colorize.mode = \"monochrome_only\""]
        );
    }

    #[test]
    fn a_byte_order_mark_does_not_make_the_file_unreadable() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("overlay.json");
        let with_bom = "\u{feff}{\"ai_feature_mode\":\"high_quality\"}";
        std::fs::write(&path, with_bom).expect("write overlay");
        let mut settings = Settings::default();
        apply(&mut settings, &OsString::from(&path), true)
            .expect("every Windows editor writes one; it must not be an error");
        assert_eq!(
            settings.ai_feature_mode,
            crate::settings::AiFeatureMode::HighQuality
        );
    }

    #[test]
    fn an_unchanged_value_is_not_reported_as_a_change() {
        let mut settings = Settings::default();
        let mode = serde_json::to_value(settings.ai_feature_mode).expect("serializable");
        let changed = apply(
            &mut settings,
            &overlay(&format!(r#"{{"ai_feature_mode":{mode}}}"#)),
            true,
        )
        .expect("overlay should apply");
        assert!(
            changed.is_empty(),
            "restating the current value is not a change: {changed:?}"
        );
    }

    #[test]
    fn a_misspelled_setting_stops_the_run_instead_of_being_ignored() {
        let mut settings = Settings::default();
        let error = apply(
            &mut settings,
            &overlay(r#"{"global_preset":{"colorise":{"mode":"monochrome_only"}}}"#),
            true,
        )
        .expect_err("a name that does not exist must not be silently dropped");
        match error {
            OverrideError::UnknownSetting { path } => {
                assert_eq!(
                    path, "global_preset.colorise",
                    "the report names the full path"
                )
            }
            other => panic!("expected UnknownSetting, got {other:?}"),
        }
    }

    #[test]
    fn it_refuses_to_touch_a_profile_that_is_not_isolated() {
        let mut settings = Settings::default();
        let error = apply(
            &mut settings,
            &overlay(r#"{"ai_feature_mode":"HighQuality"}"#),
            false,
        )
        .expect_err("without --data-dir this would rewrite the settings someone is using");
        assert!(matches!(error, OverrideError::NoDataDir));
    }

    /// Inline overlays must start with `{`, so a parsed non-object can only arrive from a file.
    #[test]
    fn something_that_is_not_an_object_is_named_as_such() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("overlay.json");
        std::fs::write(&path, "[1, 2]").expect("write overlay");
        let mut settings = Settings::default();
        let error = apply(&mut settings, &OsString::from(&path), true).expect_err("not an object");
        assert!(matches!(error, OverrideError::NotAnObject), "got {error:?}");
    }

    #[test]
    fn a_value_that_is_not_valid_for_the_setting_says_what_it_could_be() {
        let mut settings = Settings::default();
        let error = apply(
            &mut settings,
            &overlay(r#"{"global_preset":{"colorize":{"mode":"MonochromeOnly"}}}"#),
            true,
        )
        .expect_err("enum values are the serialized forms, not the Rust variant names");
        let OverrideError::Rejected(source) = error else {
            panic!("expected the settings to reject the value");
        };
        let message = source.to_string();
        assert!(
            message.contains("monochrome_only"),
            "the error should list what is accepted: {message}"
        );
    }
}
