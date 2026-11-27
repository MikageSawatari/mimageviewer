//! Pure mIV Sound document transformations. The caller owns I/O and the host lock.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::sound_effects;

const MAX_STATE_BYTES: usize = 4 * 1024 * 1024;
const MAX_PRESET_BYTES: usize = 8 * 1024 * 1024;
const MAX_STAGES: usize = 128;
const MAX_DSP: usize = 96;
const MAX_WARNINGS: usize = 20;
// The service wraps this projection in SoundResponse::State before applying the
// same common budget. Its bounded status/outcome fields need their own space.
const GET_WRAPPER_RESERVE_CHARS: usize = 256;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) enum Pipeline {
    A,
    B,
}

impl Pipeline {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::A => "pipelineA",
            Self::B => "pipelineB",
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EffectInput {
    pub(crate) id: Option<u32>,
    #[serde(rename = "type")]
    pub(crate) effect_type: Option<String>,
    pub(crate) enabled: Option<bool>,
    pub(crate) parameters: Option<Map<String, Value>>,
    #[serde(rename = "inputBus")]
    pub(crate) input_bus: Option<u8>,
    #[serde(rename = "outputBus")]
    pub(crate) output_bus: Option<u8>,
    pub(crate) channel: Option<String>,
}

impl EffectInput {
    fn id_only(&self) -> bool {
        self.id.is_some()
            && self.effect_type.is_none()
            && self.enabled.is_none()
            && self.parameters.is_none()
            && self.input_bus.is_none()
            && self.output_bus.is_none()
            && self.channel.is_none()
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApplyInput {
    pub(crate) pipeline: Pipeline,
    pub(crate) effects: Vec<EffectInput>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SelectInput {
    #[serde(rename = "use")]
    pub(crate) use_pipeline: Option<Pipeline>,
    pub(crate) bypass: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GetInput {
    pub(crate) pipeline: Option<Pipeline>,
    #[serde(default)]
    pub(crate) offset: usize,
    pub(crate) ids: Option<Vec<u32>>,
}

#[derive(Clone, Debug)]
pub(crate) struct GetContext {
    pub(crate) revision: Option<String>,
    pub(crate) latency_secs: f64,
    pub(crate) latency_remaining_secs: f64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ParameterWarning {
    pub(crate) id: u32,
    pub(crate) key: String,
    pub(crate) key_truncated: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedDocument {
    pub(crate) bytes: Vec<u8>,
    pub(crate) unchanged: bool,
    pub(crate) before_counts: [usize; 2],
    pub(crate) after_counts: [usize; 2],
    pub(crate) warnings: Vec<ParameterWarning>,
    pub(crate) warning_count: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedPreset {
    pub(crate) name: String,
    pub(crate) bytes: Vec<u8>,
    pub(crate) replaced: bool,
}

fn parse_state(bytes: &[u8]) -> Result<Value, String> {
    if bytes.len() >= MAX_STATE_BYTES {
        return Err("mIV Sound の状態が大きすぎます".into());
    }
    let root: Value = serde_json::from_slice(bytes).map_err(|_| "mIV Sound の状態を読めません")?;
    if root.get("formatVersion").and_then(Value::as_u64) != Some(1) {
        return Err("対応していない状態の版です。mIV を更新してください".into());
    }
    if !root["pipelineA"].is_array()
        || !(root["pipelineB"].is_null() || root["pipelineB"].is_array())
    {
        return Err("mIV Sound の並びを読めません".into());
    }
    Ok(root)
}

fn stages(root: &Value, pipeline: Pipeline) -> &[Value] {
    root[pipeline.key()]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn counts(root: &Value) -> [usize; 2] {
    [
        stages(root, Pipeline::A).len(),
        stages(root, Pipeline::B).len(),
    ]
}

fn logical_id(stage: &Value) -> Result<u32, String> {
    stage["id"]
        .as_u64()
        .and_then(|id| u32::try_from(id).ok())
        .filter(|id| *id != 0)
        .ok_or_else(|| "段の ID を読めません".into())
}

fn maximum_id(root: &Value) -> Result<u32, String> {
    let mut maximum = 0;
    for pipeline in [Pipeline::A, Pipeline::B] {
        for stage in stages(root, pipeline) {
            maximum = maximum.max(logical_id(stage)?);
        }
    }
    if let Some(automation) = root.get("automation") {
        if !automation.is_object() {
            return Err("オートメーションの状態を読めません".into());
        }
        if let Some(value) = automation.get("logicalPluginIdWatermark") {
            maximum = maximum.max(
                value
                    .as_u64()
                    .and_then(|id| u32::try_from(id).ok())
                    .ok_or("オートメーションの ID を読めません")?,
            );
        }
        if let Some(bindings) = automation.get("bindings").and_then(Value::as_array) {
            for binding in bindings {
                if let Some(value) = binding.get("pluginId") {
                    maximum = maximum.max(
                        value
                            .as_u64()
                            .and_then(|id| u32::try_from(id).ok())
                            .ok_or("オートメーションの ID を読めません")?,
                    );
                }
            }
        }
    }
    Ok(maximum)
}

fn allocate_id(maximum: &mut u32) -> Result<u32, String> {
    *maximum = maximum
        .checked_add(1)
        .ok_or("段の ID をこれ以上作れません")?;
    Ok(*maximum)
}

fn update_watermark(root: &mut Value, maximum: u32) {
    let root = root.as_object_mut().expect("validated state object");
    let automation = root.entry("automation").or_insert_with(|| json!({}));
    automation
        .as_object_mut()
        .expect("validated automation object")
        .insert("logicalPluginIdWatermark".into(), json!(maximum));
}

fn validate_document(root: &Value) -> Result<(), String> {
    let mut ids = HashSet::new();
    for pipeline in [Pipeline::A, Pipeline::B] {
        let entries = stages(root, pipeline);
        if entries.len() > MAX_STAGES {
            return Err("1 つの並びは 128 段までです".into());
        }
        let mut dsp_count = 0;
        for stage in entries {
            if !ids.insert(logical_id(stage)?) {
                return Err("A と B を通して段の ID が重複しています".into());
            }
            let effect_type = stage["type"].as_str().unwrap_or_default();
            if sound_effects::is_dsp(effect_type) {
                dsp_count += 1;
            }
        }
        if dsp_count > MAX_DSP {
            return Err("1 つの並びの DSP は 96 個までです".into());
        }
    }
    Ok(())
}

fn finish(
    before: &Value,
    after: Value,
    original: &[u8],
    warnings: Vec<ParameterWarning>,
    warning_count: usize,
) -> Result<PreparedDocument, String> {
    validate_document(&after)?;
    let unchanged = *before == after;
    let bytes = if unchanged {
        original.to_vec()
    } else {
        serde_json::to_vec(&after).map_err(|error| error.to_string())?
    };
    if bytes.len() >= MAX_STATE_BYTES {
        return Err("組み立てた状態が 4 MiB 以上になります".into());
    }
    // A real response adds only bounded host revision/status metadata. Check the
    // largest variable section here before the caller asks the host to write.
    check_budget(&json!({"warnings": warnings, "warning_count": warning_count}))?;
    Ok(PreparedDocument {
        before_counts: counts(before),
        after_counts: counts(&after),
        bytes,
        unchanged,
        warnings,
        warning_count,
    })
}

fn reserved(key: &str) -> bool {
    key.starts_with("_ui")
        || matches!(
            key,
            "nm" | "en"
                | "ib"
                | "ob"
                | "ch"
                | "name"
                | "enabled"
                | "inputBus"
                | "outputBus"
                | "channel"
                | "parameters"
                | "type"
                | "id"
        )
}

fn common_collision(old: &Map<String, Value>, input: &EffectInput) -> bool {
    [
        (input.enabled.is_some(), ["enabled", "en"]),
        (input.input_bus.is_some(), ["inputBus", "ib"]),
        (input.output_bus.is_some(), ["outputBus", "ob"]),
        (input.channel.is_some(), ["channel", "ch"]),
    ]
    .iter()
    .any(|(changed, aliases)| *changed && aliases.iter().any(|key| old.contains_key(*key)))
}

fn same_shape(expected: &Value, actual: &Value) -> bool {
    match (expected, actual) {
        (Value::Null, Value::Null)
        | (Value::Bool(_), Value::Bool(_))
        | (Value::Number(_), Value::Number(_))
        | (Value::String(_), Value::String(_)) => true,
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| same_shape(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, value)| {
                    right.get(key).is_some_and(|other| same_shape(value, other))
                })
        }
        _ => false,
    }
}

fn matches_storage_shape(shape: &Value, actual: &Value) -> bool {
    match shape["type"].as_str() {
        Some("null") => actual.is_null(),
        Some("boolean") | Some("bool") => actual.is_boolean(),
        Some("number") => actual.is_number(),
        Some("string") => actual.is_string(),
        Some("array") => shape["elements"]
            .as_array()
            .zip(actual.as_array())
            .is_some_and(|(a, b)| {
                a.len() == b.len() && a.iter().zip(b).all(|(a, b)| matches_storage_shape(a, b))
            }),
        Some("object") => shape["members"]
            .as_object()
            .zip(actual.as_object())
            .is_some_and(|(a, b)| {
                a.len() == b.len()
                    && a.iter()
                        .all(|(key, a)| b.get(key).is_some_and(|b| matches_storage_shape(a, b)))
            }),
        _ => false,
    }
}

fn truncate(value: &str, limit: usize) -> (String, bool) {
    (
        value.chars().take(limit).collect(),
        value.chars().count() > limit,
    )
}

fn nearby_types(effect_type: &str) -> String {
    // Bound work on misspelled/untrusted names; catalog names are short.
    let input: Vec<char> = effect_type.to_lowercase().chars().take(128).collect();
    let mut candidates: Vec<(usize, &str)> = sound_effects::catalog()
        .effects
        .iter()
        .map(|effect| {
            let candidate: Vec<char> = effect.effect_type.to_lowercase().chars().collect();
            let mut previous: Vec<usize> = (0..=candidate.len()).collect();
            for (row, input_char) in input.iter().enumerate() {
                let mut current = vec![row + 1; candidate.len() + 1];
                for (column, candidate_char) in candidate.iter().enumerate() {
                    current[column + 1] = (current[column] + 1)
                        .min(previous[column + 1] + 1)
                        .min(previous[column] + usize::from(input_char != candidate_char));
                }
                previous = current;
            }
            (previous[candidate.len()], effect.effect_type.as_str())
        })
        .collect();
    candidates.sort_unstable();
    candidates
        .into_iter()
        .take(3)
        .map(|(_, name)| name)
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn apply(bytes: &[u8], input: &ApplyInput) -> Result<PreparedDocument, String> {
    let before = parse_state(bytes)?;
    if input.effects.len() > MAX_STAGES {
        return Err("1 つの並びは 128 段までです".into());
    }
    let mut maximum = maximum_id(&before)?;
    let original_maximum = maximum;
    let mut seen = HashSet::new();
    let mut entries = Vec::with_capacity(input.effects.len());
    let mut warnings = Vec::new();
    let mut warning_count = 0;
    for effect_input in &input.effects {
        let mut stage = if let Some(id) = effect_input.id {
            if !seen.insert(id) {
                return Err("同じ段の ID を 2 回使えません".into());
            }
            stages(&before, input.pipeline)
                .iter()
                .find(|stage| logical_id(stage).ok() == Some(id))
                .cloned()
                .ok_or("その並びに ID の段がありません")?
        } else {
            json!({})
        };
        if effect_input.id_only() {
            entries.push(stage);
            continue;
        }
        let effect_type = effect_input
            .effect_type
            .as_deref()
            .ok_or("段を変更するときは type が必要です")?;
        let effect = sound_effects::effect(effect_type).ok_or_else(|| {
            let candidates = nearby_types(effect_type);
            format!("知らない type です。sound_effects で確かめてください。候補: {candidates}")
        })?;
        if effect_input.id.is_some() && stage["type"].as_str() != Some(effect_type) {
            return Err("ID と type が一致しません".into());
        }
        if effect_input.input_bus.is_some_and(|bus| bus > 4)
            || effect_input.output_bus.is_some_and(|bus| bus > 4)
        {
            return Err("inputBus と outputBus は 0〜4 の整数です".into());
        }
        if effect_input
            .channel
            .as_deref()
            .is_some_and(|channel| !valid_channel(channel))
        {
            return Err("channel が mIV Sound の選択肢にありません".into());
        }
        let id = match effect_input.id {
            Some(id) => id,
            None => allocate_id(&mut maximum)?,
        };
        let mut parameters = match stage.get("parameters") {
            Some(Value::Object(parameters)) => parameters.clone(),
            None => Map::new(),
            _ => return Err("段の parameters を読めません。画面で編集してください".into()),
        };
        if common_collision(&parameters, effect_input) {
            return Err("予約の鍵が共通の項目と重なっています。画面で編集してください".into());
        }
        if let Some(updates) = &effect_input.parameters {
            if updates.len() > 256 {
                return Err("1 段に渡せる鍵は 256 個までです".into());
            }
            for (key, value) in updates {
                if key.chars().count() > 64 {
                    return Err("鍵の名前は 64 文字までです".into());
                }
                if reserved(key) {
                    return Err("parameters に予約の鍵は渡せません".into());
                }
                let definition = effect.parameter(key);
                if definition.is_some_and(|parameter| parameter.external_data) {
                    return Err("外部のデータは mIV Sound の画面で選んでください".into());
                }
                let valid = if let Some(expected) = parameters.get(key) {
                    same_shape(expected, value)
                } else if let Some(expected) =
                    definition.and_then(|parameter| parameter.default.as_ref())
                {
                    same_shape(expected, value)
                } else if let Some(shape) =
                    definition.and_then(|parameter| parameter.storage_shape.as_ref())
                {
                    matches_storage_shape(shape, value)
                } else {
                    let primitive = value.is_number()
                        || value.is_boolean()
                        || value
                            .as_str()
                            .is_some_and(|text| text.chars().count() <= 64);
                    if primitive {
                        warning_count += 1;
                        if warnings.len() < MAX_WARNINGS {
                            let (key, key_truncated) = truncate(key, 40);
                            warnings.push(ParameterWarning {
                                id,
                                key,
                                key_truncated,
                            });
                        }
                    }
                    primitive
                };
                if !valid {
                    return Err(format!(
                        "parameters の {} は保存される値と形が違います",
                        truncate(key, 40).0
                    ));
                }
                parameters.insert(key.clone(), value.clone());
            }
        }
        let fields = stage.as_object_mut().ok_or("段の形を読めません")?;
        if effect_input.id.is_none() {
            fields.insert("id".into(), json!(id));
            fields.insert("type".into(), json!(effect_type));
            fields.insert("name".into(), json!(effect.name));
            fields.insert("enabled".into(), json!(true));
        }
        if let Some(value) = effect_input.enabled {
            fields.insert("enabled".into(), json!(value));
        }
        if let Some(value) = effect_input.input_bus {
            fields.insert("inputBus".into(), json!(value));
        }
        if let Some(value) = effect_input.output_bus {
            fields.insert("outputBus".into(), json!(value));
        }
        if let Some(value) = &effect_input.channel {
            fields.insert("channel".into(), json!(value));
        }
        if effect_input.parameters.is_some() || effect_input.id.is_none() {
            fields.insert("parameters".into(), Value::Object(parameters));
        }
        entries.push(stage);
    }
    let mut after = before.clone();
    after[input.pipeline.key()] = Value::Array(entries);
    if maximum != original_maximum {
        update_watermark(&mut after, maximum);
    }
    finish(&before, after, bytes, warnings, warning_count)
}

fn valid_channel(channel: &str) -> bool {
    // New selections match the bundled pipeline-routing-dialog.js. Wider
    // restored routes are retained by apply when channel is omitted.
    matches!(
        channel,
        "" | "A" | "L" | "R" | "3" | "4" | "5" | "6" | "7" | "8" | "34" | "56" | "78"
    )
}

pub(crate) fn select(bytes: &[u8], input: &SelectInput) -> Result<PreparedDocument, String> {
    let before = parse_state(bytes)?;
    let mut after = before.clone();
    if let Some(pipeline) = input.use_pipeline {
        if pipeline == Pipeline::B && before["pipelineB"].is_null() {
            let mut maximum = maximum_id(&before)?;
            let original_maximum = maximum;
            let mut cloned = stages(&before, Pipeline::A).to_vec();
            for stage in &mut cloned {
                let id = allocate_id(&mut maximum)?;
                stage
                    .as_object_mut()
                    .ok_or("段の形を読めません")?
                    .insert("id".into(), json!(id));
            }
            after["pipelineB"] = Value::Array(cloned);
            if maximum != original_maximum {
                update_watermark(&mut after, maximum);
            }
        }
        after["currentPipeline"] = json!(pipeline.name());
    }
    if let Some(bypass) = input.bypass {
        after["masterBypass"] = json!(bypass);
    }
    finish(&before, after, bytes, Vec::new(), 0)
}

fn check_budget(value: &Value) -> Result<(), String> {
    let chars =
        mimageviewer_agent_ipc::mcp_content_chars(value).map_err(|error| error.to_string())?;
    if chars > mimageviewer_agent_ipc::MAX_PAGE_CHARS - GET_WRAPPER_RESERVE_CHARS {
        return Err("応答が大きすぎます".into());
    }
    Ok(())
}

fn stage_summary(stage: &Value) -> Value {
    let effect_type = stage["type"].as_str().unwrap_or_default();
    let (effect_type, type_truncated) = truncate(effect_type, 40);
    let (name, name_truncated) = truncate(stage["name"].as_str().unwrap_or_default(), 40);
    json!({"id": stage["id"], "type": effect_type, "type_truncated": type_truncated,
        "name": name, "name_truncated": name_truncated,
        "enabled": stage.get("enabled").cloned().unwrap_or(json!(true)),
        "inputBus": stage.get("inputBus").cloned().unwrap_or(json!(0)),
        "outputBus": stage.get("outputBus").cloned().unwrap_or(json!(0)),
        "channel": stage.get("channel").cloned().unwrap_or(json!(""))})
}

pub(crate) fn get(bytes: &[u8], input: &GetInput, context: &GetContext) -> Result<Value, String> {
    let root = parse_state(bytes)?;
    if !context.latency_secs.is_finite() || !context.latency_remaining_secs.is_finite() {
        return Err("遅延の値を読めません".into());
    }
    let [a, b] = counts(&root);
    let mut response = json!({"revision": context.revision, "locked": context.revision.is_some(),
        "masterBypass": root["masterBypass"], "currentPipeline": root["currentPipeline"],
        "countA": a, "countB": b, "latency_secs": context.latency_secs,
        "latency_remaining_secs": context.latency_remaining_secs,
        "note": "読み直しの直後は、mIV Sound が値を合わせる前の値が返ることがあります"});
    if input.ids.as_ref().is_some_and(|ids| ids.len() > 8) {
        return Err("値を読める ID は 8 個までです".into());
    }
    let Some(pipeline) = input.pipeline else {
        if input.ids.is_some() || input.offset != 0 {
            return Err("pipeline を指定してください".into());
        }
        check_budget(&response)?;
        return Ok(response);
    };
    response["pipeline"] = json!(pipeline);
    response["effects"] = json!([]);
    let entries = stages(&root, pipeline);
    if let Some(ids) = &input.ids {
        let mut seen = HashSet::new();
        let mut selected = Vec::new();
        for id in ids {
            if !seen.insert(*id) {
                return Err("同じ ID を 2 回指定できません".into());
            }
            let stage = entries
                .iter()
                .find(|stage| logical_id(stage).ok() == Some(*id))
                .ok_or("その並びに ID の段がありません")?;
            let mut row = stage_summary(stage);
            row["too_long"] = json!(true);
            row["note"] = json!("大きすぎるので画面で編集してください");
            response["effects"].as_array_mut().expect("array").push(row);
            selected.push(stage);
        }
        check_budget(&response)?;
        for (index, stage) in selected.iter().enumerate() {
            let parameters: Map<String, Value> = stage["parameters"]
                .as_object()
                .into_iter()
                .flat_map(|parameters| parameters.iter())
                .filter(|(key, _)| !key.starts_with("_ui"))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            let placeholder = response["effects"][index].clone();
            let row = response["effects"][index].as_object_mut().expect("object");
            row.remove("too_long");
            row.remove("note");
            row.insert("parameters".into(), Value::Object(parameters));
            let row_chars = mimageviewer_agent_ipc::mcp_content_chars(&response["effects"][index])
                .map_err(|error| error.to_string())?;
            if row_chars > mimageviewer_agent_ipc::MAX_ITEM_CHARS
                || check_budget(&response).is_err()
            {
                response["effects"][index] = placeholder;
            }
        }
    } else {
        if input.offset > entries.len() {
            return Err("offset が段の数を超えています".into());
        }
        response["offset"] = json!(input.offset);
        response["next_offset"] = Value::Null;
        for (index, stage) in entries.iter().enumerate().skip(input.offset).take(32) {
            response["effects"]
                .as_array_mut()
                .expect("array")
                .push(stage_summary(stage));
            let next = index + 1;
            response["next_offset"] = if next < entries.len() {
                json!(next)
            } else {
                Value::Null
            };
            if check_budget(&response).is_err() {
                response["effects"].as_array_mut().expect("array").pop();
                if index == input.offset {
                    return Err("段の情報が大きすぎます。画面で編集してください".into());
                }
                response["next_offset"] = json!(index);
                break;
            }
        }
    }
    check_budget(&response)?;
    Ok(response)
}

pub(crate) fn preset_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty()
        || name.chars().any(char::is_control)
        || matches!(name, "__proto__" | "constructor" | "prototype")
    {
        return Err("プリセットの名前が使えません".into());
    }
    let name = if name.starts_with("MCP:") {
        name.to_owned()
    } else {
        format!("MCP: {name}")
    };
    if name.chars().count() > 64 || name.trim_start_matches("MCP:").trim().is_empty() {
        return Err("プリセットの名前は空にできず、MCP: を含め 64 文字までです".into());
    }
    Ok(name)
}

pub(crate) fn prepare_preset(
    state: &[u8],
    existing: Option<&[u8]>,
    pipeline: Pipeline,
    name: &str,
    replace: bool,
    timestamp: &str,
) -> Result<PreparedPreset, String> {
    let root = parse_state(state)?;
    if root[pipeline.key()].is_null() {
        return Err("その並びはまだありません".into());
    }
    let name = preset_name(name)?;
    let mut presets = match existing {
        None => Map::new(),
        Some(bytes) => {
            if bytes.len() > MAX_PRESET_BYTES {
                return Err("プリセットのファイルが 8 MiB を超えています".into());
            }
            let value: Value = serde_json::from_slice(bytes)
                .map_err(|_| "プリセットのファイルを読めません。原本を残しました")?;
            value
                .as_object()
                .cloned()
                .ok_or("プリセットのファイルが object ではありません。原本を残しました")?
        }
    };
    let replaced = presets.contains_key(&name);
    if replaced && !replace {
        return Err("同じ名前のプリセットがあります".into());
    }
    presets.insert(
        name.clone(),
        json!({"pipeline": root[pipeline.key()], "name": name, "timestamp": timestamp}),
    );
    let bytes = serde_json::to_vec(&presets).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_PRESET_BYTES {
        return Err("組み立てたプリセット全体が 8 MiB を超えています".into());
    }
    Ok(PreparedPreset {
        name,
        bytes,
        replaced,
    })
}

pub(crate) fn check_pipeline_startup(config: Option<&[u8]>) -> Result<(), String> {
    let Some(config) = config else {
        return Ok(());
    };
    let config: Value =
        serde_json::from_slice(config).map_err(|_| "mIV Sound の config.json を読めません")?;
    if !config.is_object() {
        return Err("mIV Sound の config.json を読めません".into());
    }
    if matches!(
        config["pipelineStartup"].as_str(),
        Some("preset" | "default")
    ) {
        return Err("mIV Sound の起動時の並びが preset / default です。画面で最後の並びを使う設定にしてからやり直してください".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(value: &Value) -> Vec<u8> {
        serde_json::to_vec(value).unwrap()
    }

    fn document(a: Vec<Value>, b: Value) -> Value {
        json!({"formatVersion": 1, "appVersion": "fixture", "pipelineA": a, "pipelineB": b,
            "currentPipeline": "A", "masterBypass": false, "oversampling": {"factor": 2},
            "ui": {"future": [1, 2]}, "future": {"preserve": true}})
    }

    fn known_type() -> &'static str {
        &sound_effects::catalog()
            .effects
            .iter()
            .find(|effect| effect.dsp)
            .unwrap()
            .effect_type
    }

    fn stage(id: u32) -> Value {
        let effect = sound_effects::effect(known_type()).unwrap();
        json!({"id": id, "type": known_type(), "name": effect.name, "enabled": true,
            "parameters": {}, "future_stage": {"yes": true}})
    }

    fn apply_input(pipeline: Pipeline, effects: Value) -> ApplyInput {
        serde_json::from_value(json!({"pipeline": pipeline, "effects": effects})).unwrap()
    }

    fn output(result: &PreparedDocument) -> Value {
        serde_json::from_slice(&result.bytes).unwrap()
    }

    fn context() -> GetContext {
        GetContext {
            revision: None,
            latency_secs: 0.1,
            latency_remaining_secs: 1.5,
        }
    }

    #[test]
    fn sound_document_apply_preserves_other_pipeline_unknown_and_automation_fields() {
        let mut first = stage(2);
        first["parameters"] = json!({"_uiFuture": {"enabled": true}, "future": [1, 2]});
        let mut root = document(vec![first.clone(), stage(3)], json!([stage(7)]));
        root["automation"] = json!({"logicalPluginIdWatermark": 50,
            "bindings": [{"pluginId": 81, "slot": 1, "future": "kept"}], "future": [4]});
        let result = apply(
            &bytes(&root),
            &apply_input(Pipeline::A, json!([{"id": 3}, {"id": 2}])),
        )
        .unwrap();
        let changed = output(&result);
        assert_eq!(changed["pipelineA"][1], first);
        for key in [
            "pipelineB",
            "automation",
            "oversampling",
            "ui",
            "future",
            "appVersion",
        ] {
            assert_eq!(changed[key], root[key], "{key}");
        }
        assert_eq!(result.before_counts, [2, 1]);
        assert_eq!(result.after_counts, [2, 1]);
    }

    #[test]
    fn sound_document_channel_selections_match_bundled_ui_and_preserve_omitted_routes() {
        let allowed = [
            "", "A", "L", "R", "3", "4", "5", "6", "7", "8", "34", "56", "78",
        ];
        let rejected = [
            "1", "2", "9", "10", "11", "12", "13", "14", "15", "16", "910", "1112", "1314", "1516",
            "Left", "Right", "All", "Stereo", "0", "invalid",
        ];
        let root = bytes(&document(vec![stage(1)], Value::Null));
        for channel in allowed {
            // Both new stages and updates of retained stages use this boundary.
            for identity in [
                json!({"type": known_type()}),
                json!({"id": 1, "type": known_type()}),
            ] {
                let mut effect = identity;
                effect["channel"] = json!(channel);
                let prepared = apply(&root, &apply_input(Pipeline::A, json!([effect]))).unwrap();
                assert_eq!(output(&prepared)["pipelineA"][0]["channel"], channel);
            }
        }
        for channel in rejected {
            for identity in [
                json!({"type": known_type()}),
                json!({"id": 1, "type": known_type()}),
            ] {
                let mut effect = identity;
                effect["channel"] = json!(channel);
                assert!(
                    apply(&root, &apply_input(Pipeline::A, json!([effect]))).is_err(),
                    "{channel}"
                );
            }
        }
        for channel in ["9", "16", "910", "1112", "1314", "1516"] {
            let mut old_stage = stage(1);
            old_stage["channel"] = json!(channel);
            let restored = bytes(&document(vec![old_stage], Value::Null));
            for effect in [
                json!({"id": 1}),
                json!({"id": 1, "type": known_type(), "enabled": false}),
            ] {
                let prepared =
                    apply(&restored, &apply_input(Pipeline::A, json!([effect]))).unwrap();
                assert_eq!(output(&prepared)["pipelineA"][0]["channel"], channel);
            }
        }
    }

    #[test]
    fn sound_document_ids_are_same_pipeline_unique_and_types_must_match() {
        let root = bytes(&document(vec![stage(1)], json!([stage(2)])));
        for effects in [
            json!([{"id": 2}]),
            json!([{"id": 1}, {"id": 1}]),
            json!([{"id": 1, "type": "UnknownPlugin"}]),
            json!([{"id": 1, "enabled": false}]),
        ] {
            assert!(apply(&root, &apply_input(Pipeline::A, effects)).is_err());
        }
        let mut unknown = stage(1);
        unknown["type"] = json!("FuturePlugin");
        let root = bytes(&document(vec![unknown], Value::Null));
        assert!(
            apply(&root, &apply_input(Pipeline::A, json!([{"id": 1}])))
                .unwrap()
                .unchanged
        );
    }

    #[test]
    fn sound_document_new_ids_reserve_both_pipelines_watermark_bindings_and_repeat_apply() {
        let mut root = document(vec![stage(2)], json!([stage(44)]));
        root["automation"] = json!({"logicalPluginIdWatermark": 60,
            "bindings": [{"pluginId": 83, "slot": 0}], "future": "preserved"});
        let input = apply_input(
            Pipeline::A,
            json!([{"type": known_type()}, {"type": known_type()}]),
        );
        let first = apply(&bytes(&root), &input).unwrap();
        let first_value = output(&first);
        assert_eq!(first_value["pipelineA"][0]["id"], 84);
        assert_eq!(first_value["pipelineA"][1]["id"], 85);
        assert_eq!(first_value["automation"]["logicalPluginIdWatermark"], 85);
        assert_eq!(
            first_value["automation"]["bindings"],
            root["automation"]["bindings"]
        );
        assert_eq!(first_value["automation"]["future"], "preserved");
        let second = apply(&first.bytes, &input).unwrap();
        assert_eq!(output(&second)["pipelineA"][0]["id"], 86);
        assert_eq!(output(&second)["pipelineB"], root["pipelineB"]);
        let mut exhausted = root;
        exhausted["automation"]["logicalPluginIdWatermark"] = json!(u32::MAX);
        assert!(apply(&bytes(&exhausted), &input).is_err());
    }

    #[test]
    fn sound_document_unchanged_returns_original_bytes_and_does_not_add_watermark() {
        let original = serde_json::to_vec_pretty(&document(vec![stage(1)], Value::Null)).unwrap();
        let result = apply(&original, &apply_input(Pipeline::A, json!([{"id": 1}]))).unwrap();
        assert!(result.unchanged);
        assert_eq!(result.bytes, original);
        assert!(
            select(&original, &SelectInput::default())
                .unwrap()
                .unchanged
        );
    }

    #[test]
    fn sound_document_select_uninitialized_b_copies_unknown_fields_with_fresh_ids() {
        let root = document(vec![stage(1), stage(2)], Value::Null);
        let result = select(
            &bytes(&root),
            &SelectInput {
                use_pipeline: Some(Pipeline::B),
                bypass: Some(true),
            },
        )
        .unwrap();
        let changed = output(&result);
        assert_eq!(changed["pipelineA"], root["pipelineA"]);
        assert_eq!(changed["pipelineB"][0]["id"], 3);
        assert_eq!(changed["pipelineB"][1]["id"], 4);
        assert_eq!(
            changed["pipelineB"][0]["future_stage"],
            root["pipelineA"][0]["future_stage"]
        );
        assert_eq!(changed["currentPipeline"], "B");
        assert_eq!(changed["masterBypass"], true);
        assert_eq!(changed["automation"]["logicalPluginIdWatermark"], 4);
        let written = apply(&bytes(&root), &apply_input(Pipeline::B, json!([]))).unwrap();
        assert_eq!(output(&written)["pipelineB"], json!([]));
    }

    #[test]
    fn sound_document_reserved_keys_only_at_parameter_root_and_existing_collisions_reject() {
        let mut first = stage(1);
        first["parameters"] = json!({"nested": {"enabled": true, "id": 3}, "en": false});
        let root = bytes(&document(vec![first], Value::Null));
        let valid = apply_input(
            Pipeline::A,
            json!([{"id": 1, "type": known_type(),
            "parameters": {"nested": {"enabled": false, "id": 7}}}]),
        );
        assert!(apply(&root, &valid).is_ok());
        let collision = apply_input(
            Pipeline::A,
            json!([{"id": 1, "type": known_type(), "enabled": false}]),
        );
        assert!(apply(&root, &collision).unwrap_err().contains("画面"));
        for key in [
            "nm",
            "en",
            "ib",
            "ob",
            "ch",
            "name",
            "enabled",
            "inputBus",
            "outputBus",
            "channel",
            "parameters",
            "type",
            "id",
            "_ui",
            "_uiFuture",
        ] {
            let mut params = Map::new();
            params.insert(key.into(), json!(1));
            let input = apply_input(
                Pipeline::A,
                json!([{"id": 1, "type": known_type(), "parameters": params}]),
            );
            assert!(apply(&root, &input).is_err(), "{key}");
        }
    }

    #[test]
    fn sound_document_parameter_shapes_recurse_without_range_validation() {
        let mut first = stage(1);
        first["parameters"] = json!({"nested": [{"enabled": true, "gain": 1}], "scalar": "path"});
        let root = bytes(&document(vec![first], Value::Null));
        for updates in [
            json!({"nested": [{"enabled": false, "gain": -999999}]}),
            json!({"scalar": "other"}),
        ] {
            assert!(
                apply(
                    &root,
                    &apply_input(
                        Pipeline::A,
                        json!([{"id": 1, "type": known_type(), "parameters": updates}])
                    )
                )
                .is_ok()
            );
        }
        for updates in [
            json!({"nested": [{"enabled": "no", "gain": 1}]}),
            json!({"nested": [{"gain": 1}]}),
            json!({"nested": []}),
            json!({"scalar": {"x": 1}}),
        ] {
            assert!(
                apply(
                    &root,
                    &apply_input(
                        Pipeline::A,
                        json!([{"id": 1, "type": known_type(), "parameters": updates}])
                    )
                )
                .is_err()
            );
        }
    }

    #[test]
    fn sound_document_catalog_defaults_define_shape_for_new_parameters() {
        let effect = sound_effects::catalog()
            .effects
            .iter()
            .find(|effect| {
                effect.parameters.iter().any(|parameter| {
                    !parameter.external_data
                        && !reserved(&parameter.key)
                        && parameter.default.as_ref().is_some_and(Value::is_number)
                })
            })
            .unwrap();
        let parameter = effect
            .parameters
            .iter()
            .find(|parameter| {
                !parameter.external_data
                    && !reserved(&parameter.key)
                    && parameter.default.as_ref().is_some_and(Value::is_number)
            })
            .unwrap();
        let root = bytes(&document(vec![], Value::Null));
        let mut params = Map::new();
        params.insert(parameter.key.clone(), json!(123456789));
        assert!(
            apply(
                &root,
                &apply_input(
                    Pipeline::A,
                    json!([{"type": effect.effect_type, "parameters": params}])
                )
            )
            .is_ok()
        );
        params.insert(parameter.key.clone(), json!({"bad": true}));
        assert!(
            apply(
                &root,
                &apply_input(
                    Pipeline::A,
                    json!([{"type": effect.effect_type, "parameters": params}])
                )
            )
            .is_err()
        );
    }

    #[test]
    fn sound_document_external_data_is_preserved_but_never_accepted_as_input() {
        let effect = sound_effects::catalog()
            .effects
            .iter()
            .find(|effect| {
                effect
                    .parameters
                    .iter()
                    .any(|parameter| parameter.external_data)
            })
            .unwrap();
        let parameter = effect
            .parameters
            .iter()
            .find(|parameter| parameter.external_data)
            .unwrap();
        let mut first = stage(1);
        first["type"] = json!(effect.effect_type);
        first["parameters"][&parameter.key] = json!("existing-user-data");
        let root = bytes(&document(vec![first.clone()], Value::Null));
        let kept = apply(&root, &apply_input(Pipeline::A, json!([{"id": 1}]))).unwrap();
        assert_eq!(output(&kept)["pipelineA"][0], first);
        let mut params = Map::new();
        params.insert(parameter.key.clone(), json!("existing-user-data"));
        assert!(
            apply(
                &root,
                &apply_input(
                    Pipeline::A,
                    json!([{"id": 1, "type": effect.effect_type, "parameters": params}])
                )
            )
            .is_err()
        );
    }

    #[test]
    fn sound_document_undefined_primitive_warning_count_is_bounded_and_keys_truncated() {
        let root = bytes(&document(vec![], Value::Null));
        let mut params = Map::new();
        for index in 0..30 {
            params.insert(
                format!("undefined_{index:02}_{}", "長".repeat(40)),
                json!(true),
            );
        }
        let result = apply(
            &root,
            &apply_input(
                Pipeline::A,
                json!([{"type": known_type(), "parameters": params}]),
            ),
        )
        .unwrap();
        assert_eq!(result.warning_count, 30);
        assert_eq!(result.warnings.len(), 20);
        assert!(
            result
                .warnings
                .iter()
                .all(|warning| warning.key.chars().count() == 40 && warning.key_truncated)
        );
        for value in [Value::Null, json!({}), json!([]), json!("a".repeat(65))] {
            assert!(
                apply(
                    &root,
                    &apply_input(
                        Pipeline::A,
                        json!([{"type": known_type(), "parameters": {"undefined_key": value}}])
                    )
                )
                .is_err()
            );
        }
    }

    #[test]
    fn sound_document_rejects_stage_dsp_size_format_and_duplicate_id_limits() {
        let root = bytes(&document(vec![], Value::Null));
        let effects = Value::Array((0..97).map(|_| json!({"type": known_type()})).collect());
        assert!(
            apply(&root, &apply_input(Pipeline::A, effects))
                .unwrap_err()
                .contains("96")
        );
        let unknowns = (1..=129)
            .map(|id| json!({"id": id, "type": "FuturePlugin"}))
            .collect();
        let root = bytes(&document(unknowns, Value::Null));
        assert!(
            select(&root, &SelectInput::default())
                .unwrap_err()
                .contains("128")
        );
        let mut future = document(vec![], Value::Null);
        future["formatVersion"] = json!(2);
        assert!(
            select(&bytes(&future), &SelectInput::default())
                .unwrap_err()
                .contains("更新")
        );
        let duplicate = document(vec![stage(1)], json!([stage(1)]));
        assert!(select(&bytes(&duplicate), &SelectInput::default()).is_err());
        let mut oversized = document(vec![], Value::Null);
        oversized["future"] = json!("x".repeat(MAX_STATE_BYTES));
        assert!(select(&bytes(&oversized), &SelectInput::default()).is_err());
    }

    #[test]
    fn sound_document_get_pages_truncates_unicode_and_returns_revision() {
        let mut entries: Vec<Value> = (1..=70).map(stage).collect();
        entries[0]["name"] = json!("画".repeat(41));
        entries[0]["type"] = json!("未".repeat(41));
        let root = bytes(&document(entries, Value::Null));
        let mut ctx = context();
        ctx.revision = Some("lock:12".into());
        let result = get(
            &root,
            &GetInput {
                pipeline: Some(Pipeline::A),
                ..Default::default()
            },
            &ctx,
        )
        .unwrap();
        assert_eq!(result["revision"], "lock:12");
        assert_eq!(result["effects"].as_array().unwrap().len(), 32);
        assert_eq!(result["next_offset"], 32);
        assert_eq!(
            result["effects"][0]["name"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            40
        );
        assert_eq!(result["effects"][0]["name_truncated"], true);
        assert_eq!(result["effects"][0]["type_truncated"], true);
        let next = get(
            &root,
            &GetInput {
                pipeline: Some(Pipeline::A),
                offset: 64,
                ids: None,
            },
            &context(),
        )
        .unwrap();
        assert_eq!(next["effects"].as_array().unwrap().len(), 6);
        assert!(next["next_offset"].is_null());
        assert!(next["revision"].is_null());
        assert_eq!(next["locked"], false);
        check_budget(&result).unwrap();
    }

    #[test]
    fn sound_document_get_values_are_bounded_and_ui_keys_are_hidden() {
        let mut entries: Vec<Value> = (1..=8).map(stage).collect();
        for entry in &mut entries {
            entry["parameters"] = json!({"huge": "\\\"".repeat(15000), "_uiSecret": "hidden"});
        }
        let root = bytes(&document(entries, Value::Null));
        let result = get(
            &root,
            &GetInput {
                pipeline: Some(Pipeline::A),
                offset: 0,
                ids: Some((1..=8).collect()),
            },
            &context(),
        )
        .unwrap();
        assert_eq!(result["effects"].as_array().unwrap().len(), 8);
        assert!(
            result["effects"]
                .as_array()
                .unwrap()
                .iter()
                .all(|row| row["too_long"] == true)
        );
        check_budget(&result).unwrap();
        let mut small = stage(1);
        small["parameters"] = json!({"visible": 4, "_uiSecret": "hidden"});
        let result = get(
            &bytes(&document(vec![small], Value::Null)),
            &GetInput {
                pipeline: Some(Pipeline::A),
                offset: 0,
                ids: Some(vec![1]),
            },
            &context(),
        )
        .unwrap();
        assert_eq!(result["effects"][0]["parameters"], json!({"visible": 4}));
        assert!(
            get(
                &root,
                &GetInput {
                    pipeline: Some(Pipeline::A),
                    offset: 0,
                    ids: Some((1..=9).collect())
                },
                &context()
            )
            .is_err()
        );
    }

    #[test]
    fn sound_document_preset_names_replacement_and_user_presets_are_preserved() {
        let root = bytes(&document(vec![stage(1)], Value::Null));
        let existing = bytes(&json!({"User": {"future": "keep"}, "MCP: Saved": {"old": true}}));
        let result = prepare_preset(
            &root,
            Some(&existing),
            Pipeline::A,
            " Saved ",
            true,
            "2026-11-27T00:00:00Z",
        )
        .unwrap();
        assert_eq!(result.name, "MCP: Saved");
        assert!(result.replaced);
        let value: Value = serde_json::from_slice(&result.bytes).unwrap();
        assert_eq!(value["User"], json!({"future": "keep"}));
        assert_eq!(value["MCP: Saved"]["pipeline"][0]["id"], 1);
        assert!(
            prepare_preset(&root, Some(&existing), Pipeline::A, "Saved", false, "now").is_err()
        );
        let new = prepare_preset(&root, None, Pipeline::A, "User", true, "now").unwrap();
        assert_eq!(new.name, "MCP: User");
        assert!(!new.replaced);
        for name in [
            "",
            " ",
            "MCP: ",
            "__proto__",
            "constructor",
            "prototype",
            "a\nb",
        ] {
            assert!(preset_name(name).is_err(), "{name}");
        }
        assert!(preset_name(&"a".repeat(61)).is_err());
        assert!(preset_name(&"画".repeat(59)).is_ok());
    }

    #[test]
    fn sound_document_preset_rejects_corrupt_or_oversized_full_file() {
        let root = bytes(&document(vec![stage(1)], Value::Null));
        for existing in [b"{".as_slice(), b"[]".as_slice(), b"null".as_slice()] {
            assert!(
                prepare_preset(&root, Some(existing), Pipeline::A, "New", false, "now").is_err()
            );
        }
        let base = bytes(&json!({"User": "x".repeat(MAX_PRESET_BYTES - 64)}));
        assert!(base.len() < MAX_PRESET_BYTES);
        assert!(
            prepare_preset(&root, Some(&base), Pipeline::A, "New", false, "now")
                .unwrap_err()
                .contains("全体")
        );
        assert!(prepare_preset(&root, None, Pipeline::B, "New", false, "now").is_err());
    }

    #[test]
    fn sound_document_startup_only_rejects_reloading_overrides_and_corrupt_config() {
        assert!(check_pipeline_startup(None).is_ok());
        for config in [
            json!({}),
            json!({"pipelineStartup": "last"}),
            json!({"future": true}),
        ] {
            assert!(check_pipeline_startup(Some(&bytes(&config))).is_ok());
        }
        for config in [
            json!({"pipelineStartup": "preset"}),
            json!({"pipelineStartup": "default"}),
            json!([]),
        ] {
            assert!(check_pipeline_startup(Some(&bytes(&config))).is_err());
        }
        assert!(check_pipeline_startup(Some(b"{")).is_err());
    }

    #[test]
    fn sound_document_typed_inputs_reject_unknown_fields_and_wrong_common_types() {
        for effects in [
            json!([{"type": known_type(), "enabled": 1}]),
            json!([{"type": known_type(), "inputBus": -1}]),
            json!([{"type": known_type(), "parameters": []}]),
            json!([{"type": known_type(), "name": "injected"}]),
        ] {
            assert!(
                serde_json::from_value::<ApplyInput>(json!({"pipeline": "A", "effects": effects}))
                    .is_err()
            );
        }
        let root = bytes(&document(vec![], Value::Null));
        for effects in [
            json!([{"type": known_type(), "inputBus": 5}]),
            json!([{"type": known_type(), "channel": "invalid"}]),
        ] {
            assert!(apply(&root, &apply_input(Pipeline::A, effects)).is_err());
        }
    }

    #[test]
    fn sound_document_parameter_key_and_count_limits_apply_before_writing() {
        let root = bytes(&document(vec![], Value::Null));
        let mut long_key = Map::new();
        long_key.insert("画".repeat(65), json!(true));
        let mut too_many = Map::new();
        for index in 0..257 {
            too_many.insert(format!("new_key_{index}"), json!(true));
        }
        for params in [long_key, too_many] {
            assert!(
                apply(
                    &root,
                    &apply_input(
                        Pipeline::A,
                        json!([{"type": known_type(), "parameters": params}])
                    )
                )
                .is_err()
            );
        }
    }

    #[test]
    fn sound_document_existing_shape_precedes_catalog_and_get_row_budget_is_enforced() {
        let effect = sound_effects::effect(known_type()).unwrap();
        let parameter = effect
            .parameters
            .iter()
            .find(|parameter| !parameter.external_data && !reserved(&parameter.key))
            .unwrap();
        let mut first = stage(1);
        first["parameters"][&parameter.key] = json!({"future": [true]});
        let root = bytes(&document(vec![first], Value::Null));
        let mut updates = Map::new();
        updates.insert(parameter.key.clone(), json!({"future": [false]}));
        assert!(
            apply(
                &root,
                &apply_input(
                    Pipeline::A,
                    json!([{"id": 1, "type": known_type(), "parameters": updates}])
                )
            )
            .is_ok()
        );

        let mut first = stage(1);
        first["parameters"] = json!({"long": "x".repeat(6500)});
        let result = get(
            &bytes(&document(vec![first], Value::Null)),
            &GetInput {
                pipeline: Some(Pipeline::A),
                offset: 0,
                ids: Some(vec![1]),
            },
            &context(),
        )
        .unwrap();
        assert_eq!(result["effects"][0]["too_long"], true);
        assert!(
            mimageviewer_agent_ipc::mcp_content_chars(&result).unwrap()
                <= mimageviewer_agent_ipc::MAX_PAGE_CHARS - GET_WRAPPER_RESERVE_CHARS
        );
    }

    #[test]
    fn sound_document_unknown_type_returns_nearby_catalog_names() {
        let expected = known_type();
        let misspelling = format!("{expected}x");
        assert!(nearby_types(&misspelling).contains(expected));
        let root = bytes(&document(vec![], Value::Null));
        let error = apply(
            &root,
            &apply_input(Pipeline::A, json!([{"type": misspelling}])),
        )
        .unwrap_err();
        assert!(error.contains(expected));
    }
}
