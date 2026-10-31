use super::*;
use std::io::Write;

fn permission(gps: bool) -> PermissionSnapshot {
    PermissionSnapshot {
        level: AgentLevel::ReadOnly,
        thumbnails: false,
        gps,
    }
}

fn item(path: &Path, subresource: Option<&str>) -> ItemRef {
    ItemRef {
        path: path.to_string_lossy().into_owned(),
        subresource: subresource.map(str::to_owned),
    }
}

fn png(chunks: &[(&str, &str)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, 7, 5);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    for (key, value) in chunks {
        encoder
            .add_text_chunk((*key).to_owned(), (*value).to_owned())
            .unwrap();
    }
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&[0; 7 * 5 * 3])
        .unwrap();
    bytes
}

fn zip_bytes(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let date = zip::DateTime::from_date_and_time(2020, 2, 3, 4, 5, 6).unwrap();
    for (name, bytes) in members {
        writer
            .start_file(
                *name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored)
                    .last_modified_time(date),
            )
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn seed_tags(path: &Path, tag: &str) {
    crate::tags_db::TagsDb::open()
        .unwrap()
        .set_item_tags(&crate::tags_db::item_key_for_path(path), [tag], "fixture")
        .unwrap();
}

#[test]
fn png_projection_bounds_prompt_and_preserves_file_tags_and_rating() {
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    let path = directory.path().join("long.png");
    let parameters = format!(
        "{}\nNegative prompt: bad\nSteps: 25, Sampler: Euler, CFG scale: 7, Seed: 42, Size: 7x5, Model: example",
        "cat\\\"".repeat(12000)
    );
    std::fs::write(
        &path,
        png(&[
            ("parameters", &parameters),
            ("UnknownSecret", "UNKNOWN_CHUNK_SECRET"),
            ("workflow", "WORKFLOW_SECRET"),
        ]),
    )
    .unwrap();
    seed_tags(&path, "file-tag");
    crate::rating_db::RatingDb::open()
        .unwrap()
        .set(&crate::adjustment_db::normalize_path(&path), 4)
        .unwrap();
    let output = read(
        &item(&path, None),
        permission(false),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        output.dimensions,
        InfoField::Present(Dimensions {
            width: 7,
            height: 5,
            unit: DimensionUnit::Pixels
        })
    );
    assert_eq!(output.rating, 4);
    assert_eq!(
        output.tags,
        InfoField::Present(vec!["#file-tag".to_owned()])
    );
    assert!(output.container_tags.is_empty());
    assert!(mcp_content_chars(&output).unwrap() <= MAX_ITEM_CHARS);
    let (rows, cursor, truncated) = crate::agent_ipc::bounded_rows(
        vec![output.clone()],
        0,
        false,
        "get_item_info",
        serde_json::json!([item(&path, None)]),
        "items",
    )
    .unwrap();
    let page = InfoPage {
        items: rows,
        cursor,
        truncated,
    };
    assert_eq!(page.items.len(), 1);
    assert!(page.cursor.is_none());
    assert!(mcp_content_chars(&page).unwrap() <= MAX_PAGE_CHARS);
    let InfoField::Present(generation) = &output.generation else {
        panic!("missing generation");
    };
    assert!(generation.prompt.as_ref().unwrap().truncated);
    assert_eq!(generation.seed.as_ref().unwrap().text, "42");
    let json = serde_json::to_string(&output).unwrap();
    for secret in [
        "UNKNOWN_CHUNK_SECRET",
        "WORKFLOW_SECRET",
        "\"raw\"",
        "prompt_json",
        "workflow_json",
        "UserComment",
        "sampler_name",
    ] {
        assert!(!json.contains(secret), "leaked {secret}");
    }
}

#[test]
fn unknown_chunks_and_unresolved_workflow_never_become_generation_fields() {
    let unknown = project_generation(crate::png_metadata::extract_metadata_from_bytes_for_agent(
        &png(&[("Secret", "SECRET_CHUNK")]),
    ));
    assert_eq!(unknown, InfoField::Unavailable);
    let prompt = r#"{"1":{"class_type":"CLIPTextEncode","inputs":{"text":"safe positive"}},"2":{"class_type":"CLIPTextEncode","inputs":{"text":"safe negative"}},"3":{"class_type":"KSampler","inputs":{"positive":["1",0],"negative":["2",0],"seed":12,"steps":20,"sampler_name":"euler","scheduler":"UNKNOWN_SCHEDULER_SECRET"}},"4":{"class_type":"UnknownSecret","inputs":{"text":"UNKNOWN_NODE_SECRET"}}}"#;
    let source = crate::png_metadata::extract_metadata_from_bytes_for_agent(&png(&[
        ("prompt", prompt),
        ("workflow", r#"{"secret":"WORKFLOW_SECRET"}"#),
    ]));
    let InfoField::Present(generation) = project_generation(source) else {
        panic!("missing generation");
    };
    assert_eq!(generation.prompt.as_ref().unwrap().text, "safe positive");
    assert_eq!(
        generation.negative_prompt.as_ref().unwrap().text,
        "safe negative"
    );
    assert_eq!(generation.sampler.as_ref().unwrap().text, "euler");
    let json = serde_json::to_string(&generation).unwrap();
    assert!(!json.contains("SECRET"));
    assert!(!json.contains("class_type"));
    assert!(!json.contains("prompt_json"));
}

fn scalar_generation(chunk: &str, json: &serde_json::Value) -> InfoField<GenerationInfo> {
    project_generation(crate::png_metadata::extract_metadata_from_bytes_for_agent(
        &png(&[(chunk, &json.to_string())]),
    ))
}

fn assert_no_nested_generation(source: &InfoField<GenerationInfo>) {
    let InfoField::Present(generation) = source else {
        return;
    };
    let mut values = vec![generation.tool.as_deref(), generation.model.as_deref()];
    values.extend(
        [
            &generation.prompt,
            &generation.negative_prompt,
            &generation.seed,
            &generation.steps,
            &generation.sampler,
            &generation.cfg,
            &generation.dimensions,
        ]
        .into_iter()
        .map(|field| field.as_ref().map(|value| value.text.as_str())),
    );
    for value in values.into_iter().flatten() {
        for secret in ["gps", "latitude", "workflow_json", "secret", "{"] {
            assert!(!value.contains(secret), "leaked {secret}: {value}");
        }
    }
}

#[test]
fn nested_generation_seed_is_absent_for_file_and_same_png_in_zip() {
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    let path = directory.path().join("nested.png");
    let archive = directory.path().join("nested.zip");
    let image = png(&[(
        "parameters",
        r#"{"sui_image_params":{"prompt":"cat","seed":{"gps":{"latitude":35.5},"workflow_json":{"secret":"x"}}}}"#,
    )]);
    std::fs::write(&path, &image).unwrap();
    std::fs::write(&archive, zip_bytes(&[("nested.png", &image)])).unwrap();
    for input in [item(&path, None), item(&archive, Some("nested.png"))] {
        let output = read(&input, permission(false), &AtomicBool::new(false)).unwrap();
        assert_no_nested_generation(&output.generation);
        let InfoField::Present(generation) = output.generation else {
            panic!("missing safe generation");
        };
        assert_eq!(generation.prompt.as_ref().unwrap().text, "cat");
        assert!(generation.seed.is_none());
    }
}

fn generation_from_file_and_zip(bytes: &[u8]) -> [GenerationInfo; 2] {
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    let path = directory.path().join("structured.png");
    let archive = directory.path().join("structured.zip");
    std::fs::write(&path, bytes).unwrap();
    std::fs::write(&archive, zip_bytes(&[("structured.png", bytes)])).unwrap();
    [item(&path, None), item(&archive, Some("structured.png"))].map(|input| {
        let output = read(&input, permission(false), &AtomicBool::new(false)).unwrap();
        assert_no_nested_generation(&output.generation);
        let InfoField::Present(generation) = output.generation else {
            panic!("known generation structure must retain its independent fields");
        };
        generation
    })
}

fn replace_json_chunk(
    image: &[u8],
    name: &str,
    update: impl FnOnce(&mut serde_json::Value),
) -> Vec<u8> {
    let mut chunks = crate::png_metadata::read_png_text_chunks_from_bytes(image).unwrap();
    let (_, text) = chunks.iter_mut().find(|(key, _)| key == name).unwrap();
    let mut source = serde_json::from_str(text).unwrap();
    update(&mut source);
    *text = serde_json::to_string(&source).unwrap();
    png(&chunks
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect::<Vec<_>>())
}

fn invalid_text_leaves() -> Vec<serde_json::Value> {
    use serde_json::json;
    vec![
        json!({"gps":{"latitude":35.5},"workflow_json":{"secret":"x"}}),
        json!(["secret"]),
        json!(true),
        json!(42),
        json!(null),
    ]
}

#[test]
fn known_model_object_retains_scalar_name_for_file_and_zip() {
    let image = include_bytes!("../../tests/fixtures/ai_metadata/invokeai_new.png");
    let baseline = generation_from_file_and_zip(image);
    assert_eq!(baseline[0], baseline[1]);
    assert_eq!(baseline[0].model.as_deref(), Some("juggernautXL_v9"));
    assert!(baseline[0].prompt.is_some());
    assert!(baseline[0].negative_prompt.is_some());
    for bad in invalid_text_leaves() {
        let changed = replace_json_chunk(image, "invokeai_metadata", |value| {
            value["model"]["model_name"] = bad;
        });
        let mut expected = baseline[0].clone();
        expected.model = None;
        for actual in generation_from_file_and_zip(&changed) {
            assert_eq!(actual, expected, "only the non-scalar model name is absent");
        }
    }
}

#[test]
fn known_legacy_prompt_array_retains_scalar_leaves_for_file_and_zip() {
    let image = include_bytes!("../../tests/fixtures/ai_metadata/invokeai_legacy_sdmeta.png");
    let baseline = generation_from_file_and_zip(image);
    assert_eq!(baseline[0], baseline[1]);
    assert_eq!(baseline[0].model.as_deref(), Some("deliberate_v2"));
    assert_eq!(
        baseline[0].prompt.as_ref().unwrap().text,
        "a calico cat sitting on a windowsill, watercolor, soft morning light"
    );
    for bad in invalid_text_leaves() {
        let changed = replace_json_chunk(image, "sd-metadata", |value| {
            value["image"]["prompt"][0]["prompt"] = bad;
        });
        let mut expected = baseline[0].clone();
        expected.prompt = None;
        for actual in generation_from_file_and_zip(&changed) {
            assert_eq!(
                actual, expected,
                "keep model/params and do not fall back to Dream"
            );
        }
    }
    let mixed = replace_json_chunk(image, "sd-metadata", |value| {
        value["image"]["prompt"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"prompt":"second scene","weight":1}));
    });
    let mut expected = baseline[0].clone();
    expected
        .prompt
        .as_mut()
        .unwrap()
        .text
        .push_str("\nsecond scene");
    for actual in generation_from_file_and_zip(&mixed) {
        assert_eq!(actual, expected);
    }
    // With no prompt leaf at all, keep the display parser's ordinary fallback.
    let empty = replace_json_chunk(image, "sd-metadata", |value| {
        value["image"]["prompt"] = serde_json::json!([]);
    });
    let InfoField::Present(fallback) =
        project_generation(crate::png_metadata::extract_metadata_from_bytes(&empty))
    else {
        panic!("empty legacy array should use the known Dream fallback");
    };
    assert!(fallback.prompt.is_some());
    for actual in generation_from_file_and_zip(&empty) {
        assert_eq!(actual, fallback);
    }
    let mixed = replace_json_chunk(&mixed, "sd-metadata", |value| {
        value["image"]["prompt"][0]["prompt"] = serde_json::json!({"secret":"x"});
    });
    expected.prompt.as_mut().unwrap().text = "second scene".into();
    for actual in generation_from_file_and_zip(&mixed) {
        assert_eq!(actual, expected);
    }
}

#[test]
fn known_v4_caption_structure_retains_scalar_leaves_for_file_and_zip() {
    let source = serde_json::json!({
        "v4_prompt":{"caption":{"base_caption":"base scene",
            "char_captions":[{"char_caption":"hero character"}]}},
        "v4_negative_prompt":{"caption":{"base_caption":"bad scene"}},
        "seed":45,"steps":20
    });
    // Same v4 shape as png_metadata::tests::test_detect_novelai_v4_caption_shape,
    // with no Description fallback and no raw workflow in the projected fields.
    let image = png(&[("Software", "NovelAI"), ("Comment", &source.to_string())]);
    let baseline = generation_from_file_and_zip(&image);
    assert_eq!(baseline[0], baseline[1]);
    assert_eq!(
        baseline[0].prompt.as_ref().unwrap().text,
        "base scene\nhero character"
    );
    assert_eq!(
        baseline[0].negative_prompt.as_ref().unwrap().text,
        "bad scene"
    );
    let mixed = replace_json_chunk(&image, "Comment", |value| {
        value["v4_prompt"]["caption"]["char_captions"] = serde_json::json!([
            {"char_caption":"hero character"}, "secret", {"unknown":"secret"}
        ]);
    });
    for actual in generation_from_file_and_zip(&mixed) {
        assert_eq!(
            actual, baseline[0],
            "only named caption leaves contribute text"
        );
    }
    for bad in invalid_text_leaves() {
        for (pointer, remaining) in [
            ("/v4_prompt/caption/base_caption", Some("hero character")),
            (
                "/v4_prompt/caption/char_captions/0/char_caption",
                Some("base scene"),
            ),
            ("/v4_negative_prompt/caption/base_caption", None),
        ] {
            let changed = replace_json_chunk(&image, "Comment", |value| {
                *value.pointer_mut(pointer).unwrap() = bad.clone();
            });
            let mut expected = baseline[0].clone();
            if let Some(remaining) = remaining {
                expected.prompt.as_mut().unwrap().text = remaining.into();
            } else {
                expected.negative_prompt = None;
            }
            for actual in generation_from_file_and_zip(&changed) {
                assert_eq!(
                    actual, expected,
                    "only the rejected leaf at {pointer} is absent"
                );
            }
        }
        let changed = replace_json_chunk(&image, "Comment", |value| {
            value["v4_prompt"]["caption"] = serde_json::json!({"base_caption":bad});
            value.as_object_mut().unwrap().remove("v4_negative_prompt");
        });
        let mut expected = baseline[0].clone();
        expected.prompt = None;
        expected.negative_prompt = None;
        for actual in generation_from_file_and_zip(&changed) {
            assert_eq!(
                actual, expected,
                "rejected final text leaf must not hide seed/steps"
            );
        }
    }
}

#[test]
fn every_generation_field_rejects_original_json_objects_and_arrays() {
    use serde_json::json;
    let object = json!({"gps":{"latitude":35.5},"workflow_json":{"secret":"x"}});
    for nested in [object.clone(), json!([object, "secret"])] {
        // All parameter aliases used by project_generation and all model aliases
        // recognized by these JSON formats must retain their original type check.
        for key in [
            "prompt",
            "negativeprompt",
            "model",
            "seed",
            "steps",
            "num_inference_steps",
            "sampler",
            "sampler_name",
            "cfg",
            "cfg scale",
            "cfg_scale",
            "guidance_scale",
            "guidance scale",
            "scale",
            "size",
            "dimensions",
            "width",
            "height",
        ] {
            let mut source = json!({"sui_image_params":{"prompt":"cat","negativeprompt":"bad"}});
            source["sui_image_params"][key] = nested.clone();
            let output = scalar_generation("parameters", &source);
            assert_no_nested_generation(&output);
            let InfoField::Present(generation) = output else {
                panic!("missing safe generation for {key}");
            };
            let field = match key {
                "prompt" => &generation.prompt,
                "negativeprompt" => &generation.negative_prompt,
                "seed" => &generation.seed,
                "steps" | "num_inference_steps" => &generation.steps,
                "sampler" | "sampler_name" => &generation.sampler,
                "size" | "dimensions" | "width" | "height" => &generation.dimensions,
                "model" => {
                    assert!(generation.model.is_none());
                    continue;
                }
                _ => &generation.cfg,
            };
            assert!(field.is_none(), "unexpected {key}: {field:?}");
        }
        for key in [
            "base_model",
            "base_model_name",
            "refiner_model",
            "full_prompt",
            "raw_prompt",
            "real_prompt",
            "Prompt",
            "Full prompt",
            "full_negative_prompt",
            "raw_negative_prompt",
            "real_negative_prompt",
            "Negative",
            "Negative Prompt",
        ] {
            let mut source = json!({"prompt":"cat","negative_prompt":"bad"});
            source[key] = nested.clone();
            assert_no_nested_generation(&scalar_generation("parameters", &source));
            assert_no_nested_generation(&scalar_generation("Comment", &source));
        }
        for key in [
            "positive_prompt",
            "positive",
            "prompt",
            "positive_style_prompt",
            "negative_prompt",
            "negative",
            "negative_style_prompt",
            "model",
            "model_name",
            "model_weights",
            "seed",
            "steps",
            "cfg_scale",
            "width",
            "height",
        ] {
            let mut source = json!({"positive_prompt":"cat","negative_prompt":"bad"});
            source[key] = nested.clone();
            assert_no_nested_generation(&scalar_generation("invokeai_metadata", &source));
        }
        for key in ["prompt", "negative_prompt", "negative"] {
            let mut source = json!({"image":{"prompt":"cat","negative_prompt":"bad"}});
            source["image"][key] = nested.clone();
            assert_no_nested_generation(&scalar_generation("sd-metadata", &source));
        }
        for key in [
            "prompt",
            "uc",
            "seed",
            "steps",
            "scale",
            "sampler",
            "width",
            "height",
            "v4_prompt",
            "v4_negative_prompt",
        ] {
            let mut source = json!({"prompt":"cat","uc":"bad"});
            source[key] = nested.clone();
            assert_no_nested_generation(&scalar_generation("Comment", &source));
        }
        for key in ["text", "ckpt_name", "seed", "steps", "cfg", "sampler_name"] {
            let mut source = json!({
                "1":{"class_type":"CLIPTextEncode","inputs":{"text":"cat"}},
                "2":{"class_type":"KSampler","inputs":{"positive":["1",0]}},
                "3":{"class_type":"CheckpointLoaderSimple","inputs":{}}
            });
            let node = match key {
                "text" => "1",
                "ckpt_name" => "3",
                _ => "2",
            };
            source[node]["inputs"][key] = nested.clone();
            assert_no_nested_generation(&scalar_generation("prompt", &source));
        }
        // RuinedFooocus tool detection and SwarmUI model-name/hash leaves also
        // pass through JSON-to-text conversion, even though tool is a fixed label.
        for key in ["software", "Software"] {
            let mut source = json!({"Prompt":"cat","Negative":"bad"});
            source[key] = nested.clone();
            assert_no_nested_generation(&scalar_generation("parameters", &source));
        }
        for key in ["name", "hash"] {
            let mut source =
                json!({"sui_image_params":{"prompt":"cat"},"sui_models":[{"name":"model"}]});
            source["sui_models"][0][key] = nested.clone();
            assert_no_nested_generation(&scalar_generation("parameters", &source));
        }
    }
}

#[test]
fn generation_preserves_valid_scalar_types_without_reinterpreting_text_as_json() {
    use serde_json::json;
    for seed in [json!(42), json!("42")] {
        let output = scalar_generation(
            "parameters",
            &json!({"sui_image_params":{
                "prompt":"literal {braces} and [arrays]", "negativeprompt":"bad",
                "model":"model.safetensors", "seed":seed, "steps":20, "sampler":"euler",
                "cfg":7.5, "width":7, "height":"5"
            }}),
        );
        let InfoField::Present(generation) = output else {
            panic!("missing generation")
        };
        assert_eq!(generation.tool.as_deref(), Some("SwarmUI"));
        assert_eq!(generation.model.as_deref(), Some("model"));
        assert_eq!(
            generation.prompt.as_ref().unwrap().text,
            "literal {braces} and [arrays]"
        );
        assert_eq!(generation.negative_prompt.as_ref().unwrap().text, "bad");
        assert_eq!(generation.seed.as_ref().unwrap().text, "42");
        assert_eq!(generation.steps.as_ref().unwrap().text, "20");
        assert_eq!(generation.sampler.as_ref().unwrap().text, "euler");
        assert_eq!(generation.cfg.as_ref().unwrap().text, "7.5");
        assert_eq!(generation.dimensions.as_ref().unwrap().text, "7x5");
    }
    // No currently exported generation column is boolean. Boolean flags in
    // metadata must not masquerade as numeric parameters or textual names.
    let output = scalar_generation(
        "parameters",
        &json!({"sui_image_params":{
            "prompt":"cat", "negativeprompt":true, "model":true, "sampler":true,
            "seed":true, "steps":false, "cfg":true, "width":true, "height":false
        }}),
    );
    let InfoField::Present(generation) = output else {
        panic!("missing generation")
    };
    assert!(generation.negative_prompt.is_none());
    assert!(generation.model.is_none());
    assert!(generation.sampler.is_none());
    assert!(generation.seed.is_none());
    assert!(generation.steps.is_none());
    assert!(generation.cfg.is_none());
    assert!(generation.dimensions.is_none());
}

#[test]
fn structured_chunks_never_fall_back_to_raw_generation_prompts() {
    use serde_json::json;
    let object = json!({"gps":{"latitude":35.5},"workflow_json":{"secret":"x"}});
    for source in [
        object.clone(),
        json!([object, "Steps: 20", "Sampler: euler"]),
    ] {
        for chunk in ["parameters", "Description", "Dream"] {
            let output = scalar_generation(chunk, &source);
            assert_eq!(output, InfoField::Unavailable, "{chunk}");
        }
        let output =
            project_generation(crate::png_metadata::extract_metadata_from_bytes_for_agent(
                &png(&[("prompt", "{}"), ("parameters", &source.to_string())]),
            ));
        assert_no_nested_generation(&output);
        let InfoField::Present(generation) = output else {
            panic!("missing ComfyUI")
        };
        assert!(generation.prompt.is_none());
    }
}

#[test]
fn zip_projection_uses_member_values_and_page_rating_with_separate_container_tags() {
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    let path = directory.path().join("book.zip");
    let image = png(&[("parameters", "page prompt\nSteps: 5, Seed: 123")]);
    std::fs::write(&path, zip_bytes(&[("pages/a.png", &image)])).unwrap();
    seed_tags(&path, "container-tag");
    let ratings = crate::rating_db::RatingDb::open().unwrap();
    ratings
        .set(&crate::adjustment_db::normalize_path(&path), 5)
        .unwrap();
    ratings
        .set(
            &crate::adjustment_db::zip_entry_key(&path, "pages/a.png"),
            2,
        )
        .unwrap();
    drop(ratings);
    let output = read(
        &item(&path, Some("pages/a.png")),
        permission(false),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(output.rating, 2);
    assert_eq!(output.size, InfoField::Present(image.len() as u64));
    assert_eq!(output.modified, InfoField::Present(1_580_702_706_000));
    assert_eq!(output.tags, InfoField::NotApplicable);
    assert_eq!(output.container_tags, vec!["#container-tag"]);
    assert_eq!(
        output.container_size,
        Some(std::fs::metadata(&path).unwrap().len())
    );
    assert_eq!(output.kind, ItemKind::Image);
    assert_eq!(
        output.dimensions,
        InfoField::Present(Dimensions {
            width: 7,
            height: 5,
            unit: DimensionUnit::Pixels
        })
    );
    assert!(matches!(output.generation, InfoField::Present(_)));
}

#[test]
fn bounded_zip_preserves_literal_nested_and_cp932_name_resolution() {
    let image = png(&[]);
    let nested = zip_bytes(&[("a.png", &image)]);
    let outer = zip_bytes(&[("inner.zip", &nested), ("inner.zip/a.png", &image)]);
    let result = read_zip_member(
        zip::ZipArchive::new(Cursor::new(outer)).unwrap(),
        "inner.zip/a.png",
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(result.name, "inner.zip/a.png");
    assert_eq!(result.bytes.unwrap(), image);
    let outer = zip_bytes(&[("inner.zip", &nested)]);
    let result = read_zip_member(
        zip::ZipArchive::new(Cursor::new(outer)).unwrap(),
        "inner.zip/a.png",
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(result.bytes.unwrap(), image);
    let mut cp932 = zip_bytes(&[("xxxx.png", &image)]);
    let encoded = encoding_rs::SHIFT_JIS.encode("日本.png").0;
    let indices = cp932
        .windows(8)
        .enumerate()
        .filter_map(|(i, value)| (value == b"xxxx.png").then_some(i))
        .collect::<Vec<_>>();
    for i in indices {
        cp932[i..i + 8].copy_from_slice(&encoded);
    }
    let result = read_zip_member(
        zip::ZipArchive::new(Cursor::new(cp932)).unwrap(),
        "日本.png",
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(result.name, "日本.png");
    assert_ne!(result.legacy_name, result.name);
    assert_eq!(result.bytes.unwrap(), image);
}

#[test]
fn nested_cp932_zip_rating_reads_legacy_key_without_migration_and_prefers_current() {
    fn cp932_zip(placeholder: &str, actual: &str, content: &[u8]) -> (Vec<u8>, String) {
        let mut bytes = zip_bytes(&[(placeholder, content)]);
        let encoded = encoding_rs::SHIFT_JIS.encode(actual).0;
        assert_eq!(encoded.len(), placeholder.len());
        let offsets: Vec<_> = bytes
            .windows(placeholder.len())
            .enumerate()
            .filter_map(|(i, value)| (value == placeholder.as_bytes()).then_some(i))
            .collect();
        for offset in offsets {
            bytes[offset..offset + placeholder.len()].copy_from_slice(&encoded);
        }
        let mut archive = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
        let legacy = archive.by_index_raw(0).unwrap().name().replace('\\', "/");
        (bytes, legacy)
    }
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    let path = directory.path().join("legacy.zip");
    let (inner, legacy_leaf) = cp932_zip("xxxx.png", "日本.png", &png(&[]));
    let (outer, legacy_outer) = cp932_zip("xxxx.zip", "日本.zip", &inner);
    std::fs::write(&path, outer).unwrap();
    let legacy =
        crate::adjustment_db::zip_entry_key(&path, &format!("{legacy_outer}/{legacy_leaf}"));
    let current = crate::adjustment_db::zip_entry_key(&path, "日本.zip/日本.png");
    let db = crate::rating_db::RatingDb::open().unwrap();
    db.set(&legacy, 5).unwrap();
    let before = std::fs::read(crate::rating_db::RatingDb::db_path()).unwrap();
    let output = read(
        &item(&path, Some("日本.zip/日本.png")),
        permission(false),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(output.rating, 5);
    assert_eq!(
        output.item.subresource.as_deref(),
        Some("日本.zip/日本.png")
    );
    assert_eq!(
        std::fs::read(crate::rating_db::RatingDb::db_path()).unwrap(),
        before
    );
    assert_eq!(db.get(&legacy), 5);
    assert_eq!(db.get(&current), 0);
    db.set(&current, 2).unwrap();
    let output = read(
        &item(&path, Some("日本.zip/日本.png")),
        permission(false),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(output.rating, 2);
    assert_eq!(db.get(&legacy), 5);
}

#[test]
fn oversized_zip_metadata_is_rejected_before_decompression_and_actual_read_is_capped() {
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    let path = directory.path().join("large.zip");
    let mut archive = zip_bytes(&[("a.png", b"not an image")]);
    let central = archive
        .windows(4)
        .position(|value| value == b"PK\x01\x02")
        .unwrap();
    archive[central + 24..central + 28]
        .copy_from_slice(&((MAX_ZIP_BYTES + 1) as u32).to_le_bytes());
    archive[22..26].copy_from_slice(&((MAX_ZIP_BYTES + 1) as u32).to_le_bytes());
    std::fs::write(&path, archive).unwrap();
    let output = read(
        &item(&path, Some("a.png")),
        permission(false),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(output.size, InfoField::Present(MAX_ZIP_BYTES + 1));
    assert_eq!(output.dimensions, InfoField::TooLarge);
    assert_eq!(output.exif, InfoField::TooLarge);
    assert_eq!(output.generation, InfoField::TooLarge);
    let mut actual = Cursor::new(vec![0_u8; 100]);
    assert!(
        read_capped(&mut actual, 8, &AtomicBool::new(false))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        actual.position(),
        9,
        "at most one cap-detection byte is read"
    );
    assert_eq!(
        read_capped(&mut Cursor::new(vec![0_u8; 8]), 8, &AtomicBool::new(false)).unwrap(),
        Some(vec![0; 8])
    );
}

#[test]
fn pdf_page_rating_and_container_tags_match_ui_without_starting_pdf_worker() {
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    let path = directory.path().join("book.pdf");
    std::fs::write(&path, b"fixture").unwrap();
    seed_tags(&path, "pdf-tag");
    let ratings = crate::rating_db::RatingDb::open().unwrap();
    ratings
        .set(&crate::adjustment_db::normalize_path(&path), 5)
        .unwrap();
    ratings
        .set(&crate::edit_source::page_key_for_pdf(&path, 2), 3)
        .unwrap();
    drop(ratings);
    let mut output = ItemInfo {
        item: item(&path, Some("page_2")),
        name: "Page 3".into(),
        kind: ItemKind::Image,
        size: InfoField::NotApplicable,
        modified: InfoField::NotApplicable,
        dimensions: InfoField::Unavailable,
        rating: 0,
        tags: InfoField::NotApplicable,
        container_size: Some(7),
        container_modified: None,
        container_tags: vec![],
        exif: InfoField::NotApplicable,
        generation: InfoField::NotApplicable,
    };
    enrich(
        &mut output,
        &GridItem::PdfPage {
            pdf_path: path.clone(),
            page_num: 2,
            content_type: None,
        },
        &path,
        None,
    )
    .unwrap();
    assert_eq!(output.rating, 3);
    assert_eq!(output.tags, InfoField::NotApplicable);
    assert_eq!(output.container_tags, vec!["#pdf-tag"]);
}

fn jpeg_with_gps() -> Vec<u8> {
    // Little-endian TIFF: IFD0 at 8, EXIF at 62, GPS at 92.
    let mut tiff = vec![0_u8; 170];
    tiff[..8].copy_from_slice(b"II*\0\x08\0\0\0");
    fn ifd(bytes: &mut Vec<u8>, offset: usize, entries: &[(u16, u16, u32, Vec<u8>)]) {
        bytes[offset..offset + 2].copy_from_slice(&(entries.len() as u16).to_le_bytes());
        for (index, (tag, format, count, data)) in entries.iter().enumerate() {
            let slot = offset + 2 + index * 12;
            bytes[slot..slot + 2].copy_from_slice(&tag.to_le_bytes());
            bytes[slot + 2..slot + 4].copy_from_slice(&format.to_le_bytes());
            bytes[slot + 4..slot + 8].copy_from_slice(&count.to_le_bytes());
            if data.len() <= 4 {
                bytes[slot + 8..slot + 8 + data.len()].copy_from_slice(data);
            } else {
                let location = bytes.len() as u32;
                bytes[slot + 8..slot + 12].copy_from_slice(&location.to_le_bytes());
                bytes.extend_from_slice(data);
            }
        }
    }
    fn rationals(values: &[u32]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| [value.to_le_bytes(), 1_u32.to_le_bytes()].concat())
            .collect()
    }
    ifd(
        &mut tiff,
        8,
        &[
            (0x010f, 2, 9, b"TestMake\0".to_vec()),
            (0x0110, 2, 10, b"TestModel\0".to_vec()),
            (0x8769, 4, 1, 62_u32.to_le_bytes().to_vec()),
            (0x8825, 4, 1, 92_u32.to_le_bytes().to_vec()),
        ],
    );
    let comment = b"ASCII\0\0\0USER_COMMENT_SECRET";
    ifd(
        &mut tiff,
        62,
        &[
            (0x9003, 2, 20, b"2020:02:03 04:05:06\0".to_vec()),
            (0x9286, 7, comment.len() as u32, comment.to_vec()),
        ],
    );
    ifd(
        &mut tiff,
        92,
        &[
            (1, 2, 2, b"N\0".to_vec()),
            (2, 5, 3, rationals(&[35, 30, 0])),
            (3, 2, 2, b"E\0".to_vec()),
            (4, 5, 3, rationals(&[139, 45, 0])),
            (5, 1, 1, vec![1]),
            (6, 5, 1, rationals(&[12])),
        ],
    );
    let mut jpeg = Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(7, 5)
        .write_to(&mut jpeg, image::ImageFormat::Jpeg)
        .unwrap();
    let jpeg = jpeg.into_inner();
    let mut output = vec![0xff, 0xd8, 0xff, 0xe1];
    output.extend_from_slice(&((tiff.len() + 8) as u16).to_be_bytes());
    output.extend_from_slice(b"Exif\0\0");
    output.extend_from_slice(&tiff);
    output.extend_from_slice(&jpeg[2..]);
    output
}

#[test]
fn jpeg_gps_is_permission_scoped_and_raw_user_comment_is_never_exported() {
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    let path = directory.path().join("gps.jpg");
    std::fs::write(&path, jpeg_with_gps()).unwrap();
    for gps in [false, true] {
        let output = read(&item(&path, None), permission(gps), &AtomicBool::new(false)).unwrap();
        let InfoField::Present(exif) = &output.exif else {
            panic!("missing EXIF");
        };
        assert_eq!(exif.make.as_deref(), Some("TestMake"));
        assert_eq!(exif.model.as_deref(), Some("TestModel"));
        assert_eq!(exif.captured_at.as_deref(), Some("2020:02:03 04:05:06"));
        assert_eq!(
            exif.gps,
            gps.then_some(GpsInfo {
                latitude: 35.5,
                longitude: 139.75,
                altitude: Some(-12.0)
            })
        );
        let json = serde_json::to_string(&output).unwrap();
        assert!(!json.contains("USER_COMMENT_SECRET"));
        assert!(!json.contains("UserComment"));
        if !gps {
            assert!(!json.contains("35.5"));
            assert!(!json.contains("139.75"));
        }
    }
}

#[test]
fn invalid_references_and_pre_cancel_are_rejected_without_heavy_io() {
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    let path = directory.path().join("missing.zip");
    for member in [
        "../a.png",
        "/a.png",
        "a/../../a.png",
        "C:/a.png",
        "a.txt",
        "a//b.png",
    ] {
        assert_eq!(
            read(
                &item(&path, Some(member)),
                permission(false),
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .code,
            ErrorCode::BadRequest
        );
    }
    assert_eq!(
        read(
            &ItemRef {
                path: "relative.png".into(),
                subresource: None
            },
            permission(false),
            &AtomicBool::new(false)
        )
        .unwrap_err()
        .code,
        ErrorCode::BadRequest
    );
    assert_eq!(
        read(
            &item(&path, None),
            permission(false),
            &AtomicBool::new(true)
        )
        .unwrap_err()
        .code,
        ErrorCode::Cancelled
    );
    let missing = read(
        &item(&directory.path().join("missing.png"), None),
        permission(false),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(missing.size, InfoField::Unavailable);
    assert_eq!(missing.dimensions, InfoField::Unavailable);
}
