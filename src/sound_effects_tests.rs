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

#[test]
fn sound_catalog_uses_stored_ranges_and_separate_display_conversions() {
    let parameter = |name: &str, key: &str| {
        serde_json::to_value(effect(name).unwrap().parameter(key).unwrap()).unwrap()
    };
    let tilt = parameter("TiltEQPlugin", "f0");
    assert!((tilt["min"].as_f64().unwrap() - 3.0).abs() < 1e-9);
    assert!((tilt["max"].as_f64().unwrap() - 9.9).abs() < 1e-9);
    assert_eq!(tilt["unit"], "ln(Hz)");
    assert_eq!(tilt["step"], 0.01);
    assert_eq!(tilt["display"]["formula"], "exp(v)");
    let jitter = parameter("SimpleJitterPlugin", "rj");
    assert_eq!(jitter["min"], 0);
    assert_eq!(jitter["max"], 200);
    assert_eq!(jitter["step"], 1);
    assert_eq!(jitter["display"]["formula"], "0.001*10^(v/20)");
    let balance = parameter("StereoBalancePlugin", "bl");
    assert_eq!(balance["min"], -1);
    assert_eq!(balance["max"], 1);
    assert_eq!(balance["step"], 0.01);
    assert_eq!(balance["display"]["formula"], "100*v");
    let decay = parameter("AdaptivePredictionEffectPlugin", "weightDecay");
    assert_eq!(decay["min"], 0);
    assert_eq!(decay["max"], 60);
    assert!(decay.get("display").is_none());
    for (name, key, min, max) in [
        ("CompressorPlugin", "rl", 10.0, 1000.0),
        ("ExpanderPlugin", "rl", 10.0, 1000.0),
        ("BrickwallLimiterPlugin", "sm", -3.0, 0.0),
    ] {
        let p = parameter(name, key);
        assert_eq!(p["min"].as_f64().unwrap(), min, "{name}.{key}");
        assert_eq!(p["max"].as_f64().unwrap(), max, "{name}.{key}");
    }
    for key in ["fr", "lp", "hp"] {
        let resonators = parameter("ModalResonatorPlugin", "rs");
        assert_eq!(resonators["members"][key]["unit"], "ln(Hz)");
        assert_eq!(resonators["members"][key]["display"]["formula"], "exp(v)");
    }
    for name in ["SpectrumAnalyzerPlugin", "SpectrogramPlugin"] {
        let points = parameter(name, "pt");
        assert_eq!(points["unit"], "log2 FFT size");
        assert_eq!(points["display"]["formula"], "2^v");
        assert_eq!(points["display"]["min"], 256);
        assert_eq!(points["display"]["max"], 16384);
    }
}

#[test]
fn sound_effects_response_keeps_stored_value_notes_and_nested_metadata() {
    let response = query(json!({"type":"ModalResonatorPlugin"})).unwrap();
    let note = response["range_note"].as_str().unwrap();
    for text in [
        "stored form",
        "display",
        "formula",
        "scale: log",
        "range_verified: false",
    ] {
        assert!(note.contains(text), "{text}");
    }
    let rs = response["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["key"] == "rs")
        .unwrap();
    assert_eq!(rs["members"]["fr"]["display"]["formula"], "exp(v)");
    // `value` was an upstream duplicate of the saved range. Numeric metadata
    // uses min/max throughout, including nested descriptions.
    fn no_duplicate_range(value: &Value) {
        match value {
            Value::Object(object) => {
                assert!(!object.get("value").is_some_and(Value::is_object));
                for child in object.values() {
                    no_duplicate_range(child);
                }
            }
            Value::Array(array) => array.iter().for_each(no_duplicate_range),
            _ => {}
        }
    }
    for effect in &catalog().effects {
        no_duplicate_range(&serde_json::to_value(effect).unwrap());
    }
    let computed = effect("FiveBandFIRPEQPlugin")
        .unwrap()
        .parameter("dy")
        .unwrap();
    assert_eq!(computed.metadata["range_verified"], false);
}

#[test]
fn sound_catalog_nested_ranges_follow_js_ordering_constraints() {
    let regions = effect("PhaseSelectEqPlugin")
        .unwrap()
        .parameter("regions")
        .unwrap();
    assert!(regions.dynamic_range);
    for (keys, min, max) in [
        (["ofl", "fl"], 20.0, 39999.0),
        (["fh", "ofh"], 21.0, 40000.0),
        (["opl", "pl"], 0.0, 179.0),
        (["ph", "oph"], 1.0, 180.0),
        (["obl", "bl"], -100.0, 99.0),
        (["bh", "obh"], -99.0, 100.0),
    ] {
        for key in keys {
            let member = &regions.metadata["members"][key];
            assert_eq!(member["min"].as_f64().unwrap(), min, "{key}");
            assert_eq!(member["max"].as_f64().unwrap(), max, "{key}");
            assert_eq!(member["dynamic_range"], true, "{key}");
        }
    }
    for name in ["HeadphoneEQPlugin", "TonalBalanceEQPlugin"] {
        let q = effect(name).unwrap().parameter("qa0").unwrap();
        assert!(q.dynamic_range);
        assert!(q.metadata["range_note"].as_str().unwrap().contains("shelf"));
    }
    let mapper = effect("SpatialMapperPlugin").unwrap();
    for key in ["dm", "fm", "rm"] {
        let items = &mapper.parameter(key).unwrap().metadata["items"];
        assert_eq!(items["min"], -1);
        assert_eq!(items["max"], 1);
        assert!(items.get("range_verified").is_none());
    }
}

#[test]
fn sound_catalog_display_only_describes_actual_screen_values() {
    for (name, key) in [
        ("DigitalErrorEmulatorPlugin", "be"),
        ("G726ADPCMSimulatorPlugin", "re"),
    ] {
        let parameter = effect(name).unwrap().parameter(key).unwrap();
        assert!(parameter.metadata.get("display").is_none(), "{name}.{key}");
        assert_eq!(parameter.kind, "number");
    }
    for (name, key, formula) in [
        ("TiltEQPlugin", "f0", "exp(v)"),
        ("SimpleJitterPlugin", "rj", "0.001*10^(v/20)"),
        ("StereoBalancePlugin", "bl", "100*v"),
        ("OscilloscopePlugin", "dt", "1000*v"),
        ("OscilloscopePlugin", "ho", "1000*v"),
        ("StereoMeterPlugin", "wt", "1000*v"),
        ("SpectrumAnalyzerPlugin", "pt", "2^v"),
        ("SpectrogramPlugin", "pt", "2^v"),
    ] {
        let parameter = effect(name).unwrap().parameter(key).unwrap();
        assert_eq!(
            parameter.metadata["display"]["formula"], formula,
            "{name}.{key}"
        );
    }
    let resonators = effect("ModalResonatorPlugin")
        .unwrap()
        .parameter("rs")
        .unwrap();
    for key in ["fr", "lp", "hp"] {
        assert_eq!(
            resonators.metadata["members"][key]["display"]["formula"],
            "exp(v)"
        );
    }
    fn display_count(value: &Value) -> usize {
        match value {
            Value::Object(object) => {
                usize::from(object.contains_key("display"))
                    + object.values().map(display_count).sum::<usize>()
            }
            Value::Array(array) => array.iter().map(display_count).sum(),
            _ => 0,
        }
    }
    assert_eq!(
        display_count(&serde_json::to_value(&catalog().effects).unwrap()),
        11
    );
}

#[test]
fn sound_catalog_option_values_match_saved_shapes() {
    fn check(
        metadata: &Value,
        saved_shape: &Value,
        default: &Value,
        path: &str,
        index: Option<usize>,
    ) {
        let value_type = |value: &Value| match value {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        let expected = saved_shape["type"].as_str().unwrap();
        assert_eq!(value_type(default), expected, "{path}: default / shape");
        if let Some(options) = metadata["options"].as_array() {
            for option in options {
                assert_eq!(
                    value_type(&option["value"]),
                    expected,
                    "{path}: option / shape"
                );
            }
        }
        if expected == "array" {
            for (i, child_shape) in saved_shape["elements"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
            {
                let child = metadata["elements"]
                    .get(i)
                    .or_else(|| metadata.get("items"))
                    .unwrap_or(metadata);
                check(
                    child,
                    child_shape,
                    &default[i],
                    &format!("{path}[{i}]"),
                    Some(i),
                );
            }
        } else if expected == "object" {
            for (key, child_shape) in saved_shape["members"].as_object().unwrap() {
                let child = &metadata["members"][key];
                let child = if child.is_array() {
                    &child[index.expect("per-element member metadata")]
                } else {
                    child
                };
                check(
                    child,
                    child_shape,
                    &default[key],
                    &format!("{path}.{key}"),
                    None,
                );
            }
        }
    }
    for effect in &catalog().effects {
        for parameter in &effect.parameters {
            if let (Some(saved_shape), Some(default)) =
                (&parameter.storage_shape, &parameter.default)
            {
                assert_eq!(saved_shape["type"], parameter.kind);
                check(
                    &serde_json::to_value(parameter).unwrap(),
                    saved_shape,
                    default,
                    &format!("{}.{}", effect.effect_type, parameter.key),
                    None,
                );
            }
        }
    }
    let analog = effect("AnalogMeterPlugin")
        .unwrap()
        .parameter("dc")
        .unwrap();
    assert_eq!(analog.metadata["options"][1]["value"], 1);
    assert_eq!(analog.metadata["options"][1]["label"], "DIN");
    let bass = effect("BassManagementPlugin").unwrap();
    for key in ["ro", "sl"] {
        assert!(
            bass.parameter(key).unwrap().metadata["items"]["options"]
                .as_array()
                .unwrap()
                .iter()
                .all(|option| option["value"].is_number())
        );
    }
}
