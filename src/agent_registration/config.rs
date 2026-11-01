//! Local ordered JSON representation avoids enabling serde_json/preserve_order globally.
use super::{Action, AppTarget, RegistrationCommand, RegistrationPreview, RegistrationState};
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

#[derive(Debug, Clone)]
enum Json {
    Object(Vec<(String, Json)>),
    Array(Vec<Json>),
    Scalar(serde_json::Value),
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Ordered;
        impl<'de> Visitor<'de> for Ordered {
            type Value = Json;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a JSON value")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = map.next_entry::<String, Json>()? {
                    // Duplicate keys have application-dependent meaning. Refuse to
                    // rewrite such a file instead of leaving a second registration.
                    if entries.iter().any(|(key, _)| key == &entry.0) {
                        return Err(serde::de::Error::custom("duplicate JSON key"));
                    }
                    entries.push(entry);
                }
                Ok(Json::Object(entries))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element()? {
                    values.push(value);
                }
                Ok(Json::Array(values))
            }
            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Json, E> {
                Ok(Json::Scalar(value.into()))
            }
            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Json, E> {
                Ok(Json::Scalar(value.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Json, E> {
                Ok(Json::Scalar(value.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Json, E> {
                Ok(Json::Scalar(serde_json::Value::from(value)))
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Json, E> {
                Ok(Json::Scalar(value.into()))
            }
            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Json, E> {
                Ok(Json::Scalar(value.into()))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Json, E> {
                Ok(Json::Scalar(serde_json::Value::Null))
            }
        }
        deserializer.deserialize_any(Ordered)
    }
}

impl Serialize for Json {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Object(entries) => {
                let mut map = serializer.serialize_map(Some(entries.len()))?;
                for (key, value) in entries {
                    map.serialize_entry(key, value)?;
                }
                map.end()
            }
            Self::Array(values) => {
                let mut seq = serializer.serialize_seq(Some(values.len()))?;
                for value in values {
                    seq.serialize_element(value)?;
                }
                seq.end()
            }
            Self::Scalar(value) => value.serialize(serializer),
        }
    }
}

impl Json {
    fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    fn object_mut(&mut self) -> Result<&mut Vec<(String, Json)>, String> {
        match self {
            Self::Object(entries) => Ok(entries),
            _ => Err("設定ファイルの形式を読めません".into()),
        }
    }
    fn value(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("valid JSON")
    }
}

fn parse_json(text: &str) -> Result<Json, String> {
    let json: Json = if text.trim().is_empty() {
        Json::Object(Vec::new())
    } else {
        serde_json::from_str(text).map_err(|_| "JSON の設定ファイルを読めません")?
    };
    if !matches!(json, Json::Object(_))
        || json
            .get("mcpServers")
            .is_some_and(|v| !matches!(v, Json::Object(_)))
    {
        return Err("JSON の設定ファイルの形式を読めません".into());
    }
    Ok(json)
}

fn parse_toml(text: &str) -> Result<toml_edit::DocumentMut, String> {
    // toml_edit normalizes some newline tokens. Normalize once and restore the
    // file's CRLF style for the complete serialized document at the boundary.
    let doc: toml_edit::DocumentMut = text
        .replace("\r\n", "\n")
        .parse()
        .map_err(|_| "TOML の設定ファイルを読めません")?;
    if doc
        .get("mcp_servers")
        .is_some_and(|item| !item.is_table_like())
    {
        return Err("TOML の設定ファイルの形式を読めません".into());
    }
    Ok(doc)
}

pub(super) fn state(
    target: AppTarget,
    text: &str,
    command: Option<&RegistrationCommand>,
) -> Result<RegistrationState, String> {
    let actual = match target {
        AppTarget::ClaudeDesktop => parse_json(text)?
            .get("mcpServers")
            .and_then(|v| v.get("mimageviewer"))
            .map(Json::value),
        AppTarget::ChatGpt => {
            let doc = parse_toml(text)?;
            doc.get("mcp_servers")
                .and_then(|t| t.get("mimageviewer"))
                .map(|item| {
                    let cmd = item.get("command").and_then(toml_edit::Item::as_str);
                    let args = item
                        .get("args")
                        .and_then(toml_edit::Item::as_array)
                        .map(|array| array.iter().map(|v| v.as_str()).collect::<Option<Vec<_>>>());
                    match (cmd, args) {
                        (Some(cmd), Some(Some(args))) => {
                            serde_json::json!({"command":cmd,"args":args})
                        }
                        _ => serde_json::Value::Null,
                    }
                })
        }
    };
    Ok(match actual {
        None => RegistrationState::Unregistered,
        Some(actual)
            if command.is_some_and(|cmd| {
                actual.get("command") == Some(&serde_json::json!(cmd.command))
                    && actual.get("args") == Some(&serde_json::json!(cmd.args))
            }) =>
        {
            RegistrationState::Registered
        }
        Some(_) => RegistrationState::DifferentTarget,
    })
}

pub(super) fn edit(
    target: AppTarget,
    text: &str,
    action: Action,
    command: &RegistrationCommand,
) -> Result<String, String> {
    match target {
        AppTarget::ClaudeDesktop => {
            let mut root = parse_json(text)?;
            let top = root.object_mut()?;
            let pos = top.iter().position(|(k, _)| k == "mcpServers");
            if pos.is_none() && action == Action::Remove {
                return Ok(text.into());
            }
            let pos = pos.unwrap_or_else(|| {
                top.push(("mcpServers".into(), Json::Object(Vec::new())));
                top.len() - 1
            });
            let servers = top[pos].1.object_mut()?;
            let existing = servers.iter().position(|(k, _)| k == "mimageviewer");
            match action {
                Action::Remove => {
                    servers.retain(|(k, _)| k != "mimageviewer");
                }
                Action::Register => {
                    let value = Json::Object(vec![
                        (
                            "command".into(),
                            Json::Scalar(command.command.clone().into()),
                        ),
                        (
                            "args".into(),
                            Json::Array(
                                command
                                    .args
                                    .iter()
                                    .map(|s| Json::Scalar(s.clone().into()))
                                    .collect(),
                            ),
                        ),
                    ]);
                    if let Some(index) = existing {
                        servers[index].1 = value;
                    } else {
                        servers.push(("mimageviewer".into(), value));
                    }
                }
            }
            let mut output =
                serde_json::to_string_pretty(&root).map_err(|_| "JSON の設定を作れません")?;
            output.push('\n');
            Ok(output)
        }
        AppTarget::ChatGpt => {
            let mut doc = parse_toml(text)?;
            match action {
                Action::Remove => {
                    if let Some(servers) = doc
                        .get_mut("mcp_servers")
                        .and_then(toml_edit::Item::as_table_like_mut)
                    {
                        servers.remove("mimageviewer");
                    }
                }
                Action::Register => {
                    if !doc.contains_key("mcp_servers") {
                        doc["mcp_servers"] = toml_edit::Item::Table(toml_edit::Table::new());
                    }
                    let mut server = toml_edit::Table::new();
                    server["command"] = toml_edit::value(&command.command);
                    let mut args = toml_edit::Array::new();
                    for arg in &command.args {
                        args.push(arg.as_str());
                    }
                    server["args"] = toml_edit::value(args);
                    server["default_tools_approval_mode"] = toml_edit::value("approve");
                    let server = if doc["mcp_servers"].is_inline_table() {
                        toml_edit::Item::Value(toml_edit::Value::InlineTable(
                            server.into_inline_table(),
                        ))
                    } else {
                        toml_edit::Item::Table(server)
                    };
                    doc["mcp_servers"]["mimageviewer"] = server;
                }
            }
            let output = doc.to_string();
            Ok(if text.contains("\r\n") {
                output.replace("\r\n", "\n").replace('\n', "\r\n")
            } else {
                output
            })
        }
    }
}

pub(super) fn preview(
    target: AppTarget,
    text: &str,
    action: Action,
    command: &RegistrationCommand,
) -> Result<RegistrationPreview, String> {
    let edited = edit(target, text, action, command)?;
    Ok(replacement_preview(text, &edited))
}

/// Trim only whole equal lines so indentation and line endings remain verbatim.
/// The remaining ranges describe one exact replacement, including any JSON
/// reformatting done by the editor; no registration block is reconstructed.
fn replacement_preview(original: &str, edited: &str) -> RegistrationPreview {
    let before: Vec<_> = original.split_inclusive('\n').collect();
    let after: Vec<_> = edited.split_inclusive('\n').collect();
    let prefix = before
        .iter()
        .zip(&after)
        .take_while(|(before, after)| before == after)
        .count();
    let suffix = before[prefix..]
        .iter()
        .rev()
        .zip(after[prefix..].iter().rev())
        .take_while(|(before, after)| before == after)
        .count();
    RegistrationPreview {
        removed: before[prefix..before.len() - suffix].concat(),
        added: after[prefix..after.len() - suffix].concat(),
    }
}
