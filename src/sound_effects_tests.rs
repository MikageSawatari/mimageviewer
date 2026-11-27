use super::*;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("vendor/miv-sound/VERSION").is_file())
        .expect("repository vendor distribution")
        .to_path_buf()
}

fn js_inputs(root: &Path, directory: &Path, paths: &mut BTreeSet<String>) {
    for entry in fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        assert!(!kind.is_symlink(), "catalog inputs must be real files");
        if kind.is_dir() {
            js_inputs(root, &entry.path(), paths);
        } else if entry.path().extension().is_some_and(|ext| ext == "js") {
            paths.insert(
                entry
                    .path()
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
}

#[test]
fn sound_catalog_version_and_all_input_digests_match_distribution() {
    let root = root();
    let catalog = catalog();
    assert_eq!(catalog.schema_version, 1);
    assert_eq!(
        catalog.sound_version,
        fs::read_to_string(root.join("vendor/miv-sound/VERSION"))
            .unwrap()
            .trim()
    );
    let web = root.join("vendor/miv-sound/mIV Sound.vst3/Contents/Resources/webview");
    let mut expected = BTreeSet::new();
    js_inputs(&root, &web, &mut expected);
    expected.insert(
        "vendor/miv-sound/mIV Sound.vst3/Contents/Resources/webview/plugins/plugins.txt".into(),
    );
    expected.insert("vendor/miv-sound/VERSION".into());
    expected.insert("scripts/generate-sound-effects.mjs".into());
    let actual = catalog
        .inputs
        .iter()
        .map(|input| input.path.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(actual.len(), catalog.inputs.len(), "duplicate input path");
    assert_eq!(
        actual, expected,
        "regenerate after adding or removing a JS input"
    );
    for input in &catalog.inputs {
        let digest = format!(
            "{:x}",
            Sha256::digest(fs::read(root.join(&input.path)).unwrap())
        );
        assert_eq!(
            digest, input.sha256,
            "regenerate after changing {}",
            input.path
        );
    }
}

#[test]
fn sound_catalog_contains_addable_effects_and_preserves_native_dsp_lookup() {
    assert_eq!(catalog().effects.len(), 114);
    let mut previous = "";
    for effect in &catalog().effects {
        assert!(previous < effect.effect_type.as_str());
        previous = &effect.effect_type;
        assert_eq!(effect.dsp, is_dsp(&effect.effect_type));
        assert_eq!(
            effect
                .parameters
                .iter()
                .map(|p| &p.key)
                .collect::<BTreeSet<_>>()
                .len(),
            effect.parameters.len()
        );
        assert!(
            effect
                .parameters
                .iter()
                .all(|parameter| parameter.default.is_some() == parameter.storage_shape.is_some())
        );
        assert_eq!(effect.ui_partial, effect.ui_error.is_some());
        for parameter in &effect.parameters {
            if let Some(shape) = &parameter.storage_shape {
                assert_eq!(shape["type"], parameter.kind);
            } else {
                assert_eq!(parameter.kind, "unknown");
            }
        }
    }
    for name in [
        "NetworkSendPlugin",
        "NetworkReceivePlugin",
        "SoundFontPlayerPlugin",
        "SpeakerDistortionCorrectionPlugin",
    ] {
        assert!(effect(name).is_none());
    }
    assert!(is_dsp("SoundFontPlayerPlugin"));
    assert!(is_dsp("SpeakerDistortionCorrectionPlugin"));
    assert!(!is_dsp("SectionPlugin"));
    assert!(!is_dsp("unknown"));
}

#[test]
fn sound_catalog_includes_saved_fir_keys_external_data_and_dynamic_ranges() {
    let fir = effect("FiveBandFIRPEQPlugin").unwrap();
    for index in 0..5 {
        for prefix in ["f", "g", "q", "s", "t", "e"] {
            assert!(
                fir.parameter(&format!("{prefix}{index}"))
                    .unwrap()
                    .default
                    .is_some()
            );
        }
    }
    assert!(fir.keys_incomplete);
    for (name, keys) in [
        ("IRReverbPlugin", vec!["ir"]),
        ("SFZNotePlayerPlugin", vec!["sf"]),
        ("CrosstalkCancellationPlugin", vec!["ll", "lr", "rl", "rr"]),
        ("RoomEqPlugin", vec!["ms", "ms0", "ms15"]),
    ] {
        for key in keys {
            assert!(
                effect(name).unwrap().parameter(key).unwrap().external_data,
                "{name}.{key}"
            );
        }
    }
    assert!(
        effect("GroupDelayEqPlugin")
            .unwrap()
            .parameter("d0")
            .unwrap()
            .dynamic_range
    );
    let nested = effect("MultibandCompressorPlugin")
        .unwrap()
        .parameter("bands")
        .unwrap();
    assert_eq!(nested.storage_shape.as_ref().unwrap()["type"], "array");
    assert_eq!(
        nested.storage_shape.as_ref().unwrap()["elements"][0]["type"],
        "object"
    );
}

#[test]
fn sound_catalog_null_default_remains_distinct_from_unknown_default() {
    let present: Parameter = serde_json::from_value(json!({"key":"x","kind":"null","default":null,"storage_shape":{"type":"null"},"external_data":false,"dynamic_range":false})).unwrap();
    let missing: Parameter = serde_json::from_value(
        json!({"key":"x","kind":"unknown","external_data":false,"dynamic_range":false}),
    )
    .unwrap();
    assert_eq!(present.default, Some(Value::Null));
    assert_eq!(missing.default, None);
    assert_eq!(missing.kind, "unknown");
}

#[test]
fn sound_effects_queries_are_offline_bounded_and_page_every_parameter() {
    let first = query(json!({})).unwrap();
    let mut listed = Vec::new();
    for page in 1..=first["pages"].as_u64().unwrap() {
        let result = query(json!({"page":page})).unwrap();
        assert!(within_budget(&result));
        listed.extend(
            result["effects"]
                .as_array()
                .unwrap()
                .iter()
                .map(|effect| effect["type"].as_str().unwrap().to_owned()),
        );
    }
    assert_eq!(
        listed,
        catalog()
            .effects
            .iter()
            .map(|effect| effect.effect_type.clone())
            .collect::<Vec<_>>()
    );
    let found = query(json!({"query":"RESTORATION noise"})).unwrap();
    assert!(
        found["effects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|effect| effect["type"] == "NoiseReductionPlugin")
    );
    assert_eq!(
        query(json!({"query":"NO_MATCH_SENTINEL"})).unwrap()["total"],
        0
    );
    for effect in &catalog().effects {
        let first = query(json!({"type":effect.effect_type})).unwrap();
        let mut keys = Vec::new();
        for page in 1..=first["pages"].as_u64().unwrap() {
            let result = call(json!({"type":effect.effect_type,"page":page})).unwrap();
            assert!(
                within_budget(&result["structuredContent"]),
                "{} page {page}",
                effect.effect_type
            );
            assert_eq!(
                serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap())
                    .unwrap(),
                result["structuredContent"]
            );
            keys.extend(
                result["structuredContent"]["parameters"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|parameter| parameter["key"].as_str().unwrap().to_owned()),
            );
        }
        assert_eq!(
            keys,
            effect
                .parameters
                .iter()
                .map(|p| p.key.clone())
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn sound_effects_rejects_invalid_arguments_without_echoing_input() {
    for args in [
        json!({"page":0}),
        json!({"page":u32::MAX}),
        json!({"page":-1}),
        json!({"query":"x".repeat(201)}),
        json!({"query":"a b c d e f g h i"}),
        json!({"type":"unknown"}),
        json!({"type":"VolumePlugin","query":"volume"}),
        json!({"type":"VolumePlugin","page":u32::MAX}),
        json!({"extra":true}),
    ] {
        assert!(query(args).is_err());
    }
}
