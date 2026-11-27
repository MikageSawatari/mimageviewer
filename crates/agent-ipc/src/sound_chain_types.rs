//! mIV Sound's editing contract. These types belong only to the v9 command;
//! existing adapters never receive the new outcome or error variants.
use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const MAX_SOUND_STAGES: usize = 128;
pub const MAX_SOUND_IDS: usize = 8;
pub const MAX_SOUND_PARAMETERS: usize = 256;
pub const MAX_SOUND_PARAMETER_KEY_CHARS: usize = 64;
pub const MAX_SOUND_PRESET_NAME_CHARS: usize = 64;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SoundPipeline {
    A,
    B,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SoundEffectInput {
    pub id: Option<u32>,
    #[serde(rename = "type")]
    pub effect_type: Option<String>,
    pub enabled: Option<bool>,
    pub parameters: Option<Map<String, Value>>,
    #[serde(rename = "inputBus")]
    pub input_bus: Option<u8>,
    #[serde(rename = "outputBus")]
    pub output_bus: Option<u8>,
    pub channel: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum SoundChainRequest {
    Lock {},
    Unlock {
        lock: String,
    },
    Get {
        pipeline: Option<SoundPipeline>,
        #[serde(default)]
        offset: usize,
        ids: Option<Vec<u32>>,
    },
    Apply {
        lock: String,
        revision: String,
        pipeline: SoundPipeline,
        effects: Vec<SoundEffectInput>,
    },
    Select {
        lock: String,
        revision: String,
        #[serde(rename = "use")]
        use_pipeline: Option<SoundPipeline>,
        bypass: Option<bool>,
    },
    Undo {
        lock: String,
    },
    SavePreset {
        lock: String,
        pipeline: SoundPipeline,
        name: String,
        #[serde(default)]
        replace: bool,
    },
}

impl SoundChainRequest {
    pub fn is_read(&self) -> bool {
        matches!(self, Self::Get { .. })
    }

    /// Validate shape at both the MCP and core boundaries. Catalog membership,
    /// current IDs, reserved parameters and document limits belong to the worker.
    pub fn validate(&self) -> Result<(), String> {
        let lock = match self {
            Self::Lock {} | Self::Get { .. } => None,
            Self::Unlock { lock }
            | Self::Apply { lock, .. }
            | Self::Select { lock, .. }
            | Self::Undo { lock }
            | Self::SavePreset { lock, .. } => Some(lock),
        };
        if lock.is_some_and(|lock| lock.is_empty()) {
            return Err("lock を指定してください".into());
        }
        match self {
            Self::Get {
                pipeline,
                offset,
                ids,
            } => {
                if pipeline.is_none() && (*offset != 0 || ids.is_some()) {
                    return Err("offset と ids には pipeline が必要です".into());
                }
                if ids.is_some() && *offset != 0 {
                    return Err("ids と offset は併用できません".into());
                }
                if let Some(ids) = ids {
                    let unique: HashSet<_> = ids.iter().collect();
                    if ids.is_empty()
                        || ids.len() > MAX_SOUND_IDS
                        || ids.contains(&0)
                        || unique.len() != ids.len()
                    {
                        return Err("ids は重複しない正の整数を 1～8 個で指定してください".into());
                    }
                }
            }
            Self::Apply {
                revision, effects, ..
            } => {
                if revision.is_empty() {
                    return Err("revision を指定してください".into());
                }
                if effects.len() > MAX_SOUND_STAGES {
                    return Err("effects は 128 段までです".into());
                }
                let mut ids = HashSet::new();
                for effect in effects {
                    if let Some(id) = effect.id {
                        if id == 0 || !ids.insert(id) {
                            return Err("段の id は重複しない正の整数です".into());
                        }
                    }
                    let id_only = effect.id.is_some()
                        && effect.effect_type.is_none()
                        && effect.enabled.is_none()
                        && effect.parameters.is_none()
                        && effect.input_bus.is_none()
                        && effect.output_bus.is_none()
                        && effect.channel.is_none();
                    if !id_only && effect.effect_type.as_ref().is_none_or(String::is_empty) {
                        return Err("追加・変更する段には type が必要です".into());
                    }
                    if effect.input_bus.is_some_and(|bus| bus > 4)
                        || effect.output_bus.is_some_and(|bus| bus > 4)
                    {
                        return Err("inputBus と outputBus は 0～4 です".into());
                    }
                    if effect.parameters.as_ref().is_some_and(|parameters| {
                        parameters.len() > MAX_SOUND_PARAMETERS
                            || parameters
                                .keys()
                                .any(|key| key.chars().count() > MAX_SOUND_PARAMETER_KEY_CHARS)
                    }) {
                        return Err("parameters は 256 鍵まで、鍵の名前は 64 文字までです".into());
                    }
                }
            }
            Self::Select {
                revision,
                use_pipeline,
                bypass,
                ..
            } => {
                if revision.is_empty() {
                    return Err("revision を指定してください".into());
                }
                if use_pipeline.is_none() && bypass.is_none() {
                    return Err("use または bypass を指定してください".into());
                }
            }
            Self::SavePreset { name, .. } => {
                let trimmed = name.trim();
                let length =
                    trimmed.chars().count() + if trimmed.starts_with("MCP:") { 0 } else { 5 };
                if trimmed.is_empty()
                    || length > MAX_SOUND_PRESET_NAME_CHARS
                    || trimmed.chars().any(char::is_control)
                    || matches!(trimmed, "__proto__" | "constructor" | "prototype")
                    || trimmed.trim_start_matches("MCP:").trim().is_empty()
                {
                    return Err("プリセット名は MCP: を含め 1～64 文字の有効な名前です".into());
                }
            }
            Self::Lock {} | Self::Unlock { .. } | Self::Undo { .. } => {}
        }
        Ok(())
    }
}

/// The caller parses the raw JSON with parse_unique_json before making a Value.
pub fn parse_sound_chain_request(value: Value) -> Result<SoundChainRequest, String> {
    let request: SoundChainRequest =
        serde_json::from_value(value).map_err(|error| error.to_string())?;
    request.validate()?;
    Ok(request)
}

pub type SoundChainResult = Result<SoundChainResponse, SoundChainError>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SoundChainErrorCode {
    Busy,
    Unlocked,
    LockMismatch,
    Changed,
    NothingToUndo,
    Aborted,
    Invalid,
    Unavailable,
}

impl SoundChainErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Busy => "busy",
            Self::Unlocked => "unlocked",
            Self::LockMismatch => "lock_mismatch",
            Self::Changed => "changed",
            Self::NothingToUndo => "nothing_to_undo",
            Self::Aborted => "aborted",
            Self::Invalid => "invalid",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SoundChainError {
    pub code: SoundChainErrorCode,
    pub message: String,
}

impl std::fmt::Display for SoundChainError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SoundChainError {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SoundChainResponse {
    pub outcome: SoundChainOutcome,
    pub persistence_warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SoundChainOutcome {
    Locked {
        lock: String,
        revision: String,
    },
    Released {
        already_released: bool,
    },
    State {
        state: SoundChainState,
    },
    Accepted {
        revision: String,
        counts: [usize; 2],
        can_undo: bool,
        warnings: Vec<SoundParameterWarning>,
        warning_count: usize,
        note: String,
    },
    Unchanged {
        revision: String,
    },
    Undone {
        revision: String,
    },
    PresetSaved {
        name: String,
        replaced: bool,
        revision: String,
        note: String,
    },
    Pending {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SoundParameterWarning {
    pub id: u32,
    pub key: String,
    pub key_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SoundChainState {
    pub revision: Option<String>,
    pub locked: bool,
    #[serde(rename = "masterBypass")]
    pub master_bypass: bool,
    #[serde(rename = "currentPipeline")]
    pub current_pipeline: SoundPipeline,
    #[serde(rename = "countA")]
    pub count_a: usize,
    #[serde(rename = "countB")]
    pub count_b: usize,
    pub latency_secs: f64,
    pub latency_remaining_secs: f64,
    pub note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipeline: Option<SoundPipeline>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effects: Option<Vec<SoundEffectState>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    // Preserve the explicit null at the end of a list without adding this key
    // to a get summary or an ids response.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_offset"
    )]
    pub next_offset: Option<Option<usize>>,
}

fn present_offset<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<usize>>, D::Error> {
    Option::<usize>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SoundEffectState {
    pub id: u32,
    #[serde(rename = "type")]
    pub effect_type: String,
    pub type_truncated: bool,
    pub name: String,
    pub name_truncated: bool,
    pub enabled: bool,
    #[serde(rename = "inputBus")]
    pub input_bus: u8,
    #[serde(rename = "outputBus")]
    pub output_bus: u8,
    pub channel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub too_long: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}
