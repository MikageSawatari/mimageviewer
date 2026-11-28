//! Version-matched, public mIV Sound catalog shared by core and the offline MCP
//! adapter. No backend, settings, audio host, or user data is accessed here.
use mimageviewer_agent_ipc::{MAX_PAGE_CHARS, mcp_content_chars};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::OnceLock};

const BUNDLED: &str = include_str!("sound_effects.generated.json");
const LIST_PAGE_SIZE: usize = 24;
const PARAM_PAGE_SIZE: usize = 8;

#[derive(Debug, Deserialize)]
pub struct Catalog {
    pub schema_version: u32,
    pub sound_version: String,
    pub native_dsp_types: Vec<String>,
    pub inputs: Vec<CatalogInput>,
    pub range_note: String,
    pub external_data_note: String,
    pub effects: Vec<Effect>,
}

#[derive(Debug, Deserialize)]
pub struct CatalogInput {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Effect {
    #[serde(rename = "type")]
    pub effect_type: String,
    pub name: String,
    pub category: String,
    pub description: String,
    pub dsp: bool,
    pub keys_incomplete: bool,
    pub ui_partial: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_error: Option<String>,
    pub parameters: Vec<Parameter>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Parameter {
    pub key: String,
    pub kind: String,
    // Some(Null) is a known null default; None means no saved default was found.
    #[serde(
        default,
        deserialize_with = "present_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_shape: Option<Value>,
    pub external_data: bool,
    pub dynamic_range: bool,
    #[serde(flatten)]
    pub metadata: BTreeMap<String, Value>,
}

fn present_value<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

impl Effect {
    pub fn parameter(&self, key: &str) -> Option<&Parameter> {
        self.parameters
            .iter()
            .find(|parameter| parameter.key == key)
    }
}

pub fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG
        .get_or_init(|| serde_json::from_str(BUNDLED).expect("valid bundled sound effects catalog"))
}

pub fn effect(effect_type: &str) -> Option<&'static Effect> {
    catalog()
        .effects
        .iter()
        .find(|effect| effect.effect_type == effect_type)
}

/// Includes native DSP effects retained in existing state but excluded from the
/// add-effect list (SoundFont Player and Distortion Correction).
pub fn is_dsp(effect_type: &str) -> bool {
    catalog()
        .native_dsp_types
        .iter()
        .any(|name| name == effect_type)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QueryArgs {
    query: Option<String>,
    #[serde(rename = "type")]
    effect_type: Option<String>,
    page: Option<u32>,
}

fn summary(effect: &Effect) -> Value {
    json!({"type":effect.effect_type,"name":effect.name,"category":effect.category,
        "description":effect.description,"keys_incomplete":effect.keys_incomplete})
}

fn detail(effect: &Effect, parameters: &[Value], page: usize, pages: usize) -> Value {
    let mut result = summary(effect);
    result["sound_version"] = json!(catalog().sound_version);
    result["range_note"] = json!(catalog().range_note);
    result["external_data_note"] = json!(catalog().external_data_note);
    result["parameter_count"] = json!(effect.parameters.len());
    result["parameters"] = json!(parameters);
    result["page"] = json!(page);
    result["pages"] = json!(pages);
    result
}

fn within_budget(value: &Value) -> bool {
    mcp_content_chars(value).is_ok_and(|chars| chars <= MAX_PAGE_CHARS)
}

fn parameter_pages(effect: &Effect) -> Result<Vec<Vec<Value>>, &'static str> {
    let mut pages = vec![Vec::new()];
    for parameter in &effect.parameters {
        let parameter = serde_json::to_value(parameter).map_err(|_| "Invalid bundled parameter")?;
        let current = pages.last_mut().expect("at least one page");
        current.push(parameter.clone());
        // Reserve the maximum page digits while partitioning. Both the text and
        // structuredContent copies are included in the common budget helper.
        if current.len() > PARAM_PAGE_SIZE
            || !within_budget(&detail(effect, current, usize::MAX, usize::MAX))
        {
            current.pop();
            if current.is_empty() {
                return Err("Bundled parameter exceeds the response budget");
            }
            if !within_budget(&detail(
                effect,
                std::slice::from_ref(&parameter),
                usize::MAX,
                usize::MAX,
            )) {
                return Err("Bundled parameter exceeds the response budget");
            }
            pages.push(vec![parameter]);
        }
    }
    Ok(pages)
}

/// Implements sound_effects without opening the core IPC connection. Pages start
/// at one. A type selects parameter detail; query filters names/categories and
/// the one-line descriptions with AND terms and a stable type-name ordering.
pub fn query(args: Value) -> Result<Value, &'static str> {
    let args: QueryArgs = serde_json::from_value(args).map_err(|_| "Invalid tool arguments")?;
    let page = args.page.unwrap_or(1);
    if page == 0 {
        return Err("Sound effects pages start at 1");
    }
    let query = args.query.unwrap_or_default();
    if query.chars().take(201).count() > 200 {
        return Err("Sound effects query must be at most 200 characters");
    }
    let terms = query
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    if terms.len() > 8 {
        return Err("Specify at most 8 sound effects query terms");
    }
    if let Some(effect_type) = args.effect_type {
        if !terms.is_empty() {
            return Err("Specify either query or type");
        }
        let effect = effect(&effect_type).ok_or("Sound effect type does not exist")?;
        let pages = parameter_pages(effect)?;
        let parameters = pages
            .get((page - 1) as usize)
            .ok_or("Sound effects page does not exist")?;
        return Ok(detail(effect, parameters, page as usize, pages.len()));
    }
    let effects = catalog()
        .effects
        .iter()
        .filter(|effect| {
            let text = format!(
                "{} {} {} {}",
                effect.effect_type, effect.name, effect.category, effect.description
            )
            .to_ascii_lowercase();
            terms.iter().all(|term| text.contains(term))
        })
        .collect::<Vec<_>>();
    let pages = effects.len().div_ceil(LIST_PAGE_SIZE).max(1);
    if page as usize > pages {
        return Err("Sound effects page does not exist");
    }
    let start = (page as usize - 1) * LIST_PAGE_SIZE;
    let result = json!({"sound_version":catalog().sound_version,"page":page,"pages":pages,"total":effects.len(),
        "effects":effects.iter().skip(start).take(LIST_PAGE_SIZE).map(|effect| summary(effect)).collect::<Vec<_>>()});
    if !within_budget(&result) {
        return Err("Bundled sound effects list exceeds the response budget");
    }
    Ok(result)
}

/// MCP envelope for the eventual tool registration; kept separate from routing.
pub fn call(args: Value) -> Result<Value, &'static str> {
    let value = query(args)?;
    Ok(
        json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":false}),
    )
}

#[cfg(test)]
#[path = "sound_effects_tests.rs"]
mod tests;
