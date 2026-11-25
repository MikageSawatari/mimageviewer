//! In-memory operation views and atomic edit preparation, independent of App.
use crate::keymap::{
    self, BindingConflict, BindingConflictKind, Chord, KeyAction, KeyBindingOverride, KeyTrigger,
    Keymap, KeymapSettings,
};
use crate::operation_customize_share::OperationCustomizeBundle;
use crate::ring_shortcut::*;
use mimageviewer_agent_ipc::*;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

fn bad(message: impl Into<String>) -> AgentError {
    AgentError::new(ErrorCode::BadRequest, message)
}

fn action(name: &str) -> Result<KeyAction, AgentError> {
    let parsed =
        KeyAction::parse_ini_name(name).ok_or_else(|| bad(format!("{name}: 不明な操作です")))?;
    if parsed.ini_name() != name {
        return Err(bad(format!(
            "{name}: 操作名は {} と指定してください",
            parsed.ini_name()
        )));
    }
    if !parsed.is_user_facing() {
        return Err(bad(format!("{name}: 利用者に公開していない操作です")));
    }
    Ok(parsed)
}

fn context_name(context: RightDragContext) -> &'static str {
    match context {
        RightDragContext::Grid => "grid",
        RightDragContext::ImageFullscreen => "image",
        RightDragContext::VideoFullscreen => "video",
        RightDragContext::EditMode => "edit",
    }
}

fn drag_context(context: RingShortcutContext) -> RightDragContext {
    match context {
        RingShortcutContext::Grid => RightDragContext::Grid,
        RingShortcutContext::ImageFullscreen => RightDragContext::ImageFullscreen,
        RingShortcutContext::VideoFullscreen => RightDragContext::VideoFullscreen,
    }
}

fn direction_name(direction: RingDirection) -> &'static str {
    match direction {
        RingDirection::Up => "up",
        RingDirection::UpRight => "up_right",
        RingDirection::Right => "right",
        RingDirection::DownRight => "down_right",
        RingDirection::Down => "down",
        RingDirection::DownLeft => "down_left",
        RingDirection::Left => "left",
        RingDirection::UpLeft => "up_left",
    }
}

#[derive(Clone, Copy, Debug)]
enum OptionSlot {
    SelectGrid,
    RingHelp,
    GestureHelp,
    GestureToast,
    Gamepad,
    VideoWheel,
}

#[derive(Clone, Copy, Debug)]
enum Slot {
    Ring(RingShortcutContext, RingDirection),
    Mouse(RingShortcutContext, MouseButtonSlot),
    Gesture(RightDragContext),
    RightDrag(RightDragContext),
    ShortClick(RightDragContext),
    Option(OptionSlot),
}

impl Slot {
    fn all() -> Vec<Self> {
        let mut slots = Vec::new();
        for &context in RingShortcutContext::all() {
            slots.extend(
                RingDirection::all()
                    .iter()
                    .map(|direction| Self::Ring(context, *direction)),
            );
            slots.extend(
                [
                    MouseButtonSlot::Back,
                    MouseButtonSlot::Forward,
                    MouseButtonSlot::Middle,
                ]
                .map(|button| Self::Mouse(context, button)),
            );
        }
        for &context in RightDragContext::all() {
            slots.extend([Self::Gesture(context), Self::RightDrag(context)]);
            if context != RightDragContext::Grid && context != RightDragContext::EditMode {
                slots.push(Self::ShortClick(context));
            }
        }
        slots.extend(
            [
                OptionSlot::SelectGrid,
                OptionSlot::RingHelp,
                OptionSlot::GestureHelp,
                OptionSlot::GestureToast,
                OptionSlot::Gamepad,
                OptionSlot::VideoWheel,
            ]
            .map(Self::Option),
        );
        slots
    }

    fn parse(name: &str) -> Result<Self, AgentError> {
        Self::all()
            .into_iter()
            .find(|slot| slot.name() == name)
            .ok_or_else(|| {
                bad(format!(
                    "{name}: 不明なスロットです。get_operations の inputs で名前を確かめてください"
                ))
            })
    }

    fn name(self) -> String {
        match self {
            Self::Ring(context, direction) => format!(
                "ring.{}.{}",
                context_name(drag_context(context)),
                direction_name(direction)
            ),
            Self::Mouse(context, button) => format!(
                "mouse.{}.{}",
                context_name(drag_context(context)),
                button.log_name()
            ),
            Self::Gesture(context) => format!("gesture.{}", context_name(context)),
            Self::RightDrag(context) => format!("right_drag.{}", context_name(context)),
            Self::ShortClick(context) => format!("short_right_click.{}", context_name(context)),
            Self::Option(option) => format!(
                "option.{}",
                match option {
                    OptionSlot::SelectGrid => "select_grid_item_on_right_drag_start",
                    OptionSlot::RingHelp => "mouse_ring_help_visible",
                    OptionSlot::GestureHelp => "mouse_gesture_help_visible",
                    OptionSlot::GestureToast => "mouse_gesture_result_toast_visible",
                    OptionSlot::Gamepad => "gamepad_enabled",
                    OptionSlot::VideoWheel => "video_normal_wheel_action",
                }
            ),
        }
    }

    fn label(self) -> String {
        match self {
            Self::Ring(context, direction) => {
                format!("リング ({}) {}", context.label(), direction.label())
            }
            Self::Mouse(context, button) => {
                format!("マウス ({}) {}", context.label(), button.label())
            }
            Self::Gesture(context) => format!("ジェスチャー ({})", context.label()),
            Self::RightDrag(context) => format!("右ドラッグ ({})", context.label()),
            Self::ShortClick(context) => format!("短い右クリック ({})", context.label()),
            Self::Option(option) => match option {
                OptionSlot::SelectGrid => "右ドラッグ開始時に一覧の項目を選ぶ",
                OptionSlot::RingHelp => "リングの案内を表示",
                OptionSlot::GestureHelp => "ジェスチャーの案内を表示",
                OptionSlot::GestureToast => "ジェスチャーの結果を通知",
                OptionSlot::Gamepad => "ゲームパッドを使う",
                OptionSlot::VideoWheel => "動画・音声のホイール",
            }
            .into(),
        }
    }

    fn value(self, bundle: &OperationCustomizeBundle) -> Value {
        let ring = &bundle.ring_shortcuts;
        match self {
            Self::Ring(context, direction) => json!(ring.profile(context).slots.get(direction.slot_index()).unwrap_or(&RingActionId::None)),
            Self::Mouse(context, button) => json!(ring.mouse_button_profile(context).action(button)),
            Self::Gesture(context) => json!(ring.mouse_gesture_profile(context).bindings.iter().map(|binding| {
                json!({"pattern": pattern_name(&binding.pattern), "action": binding.action.as_str()})
            }).collect::<Vec<_>>()),
            Self::RightDrag(context) => json!(ring.right_drag_mode(context)),
            Self::ShortClick(context) => json!(ring.viewer_short_right_click_action(context)),
            Self::Option(option) => match option {
                OptionSlot::SelectGrid => json!(ring.select_grid_item_on_right_drag_start),
                OptionSlot::RingHelp => json!(ring.mouse_ring_help_visible),
                OptionSlot::GestureHelp => json!(ring.mouse_gesture_help_visible),
                OptionSlot::GestureToast => json!(ring.mouse_gesture_result_toast_visible),
                OptionSlot::Gamepad => json!(bundle.gamepad_enabled),
                OptionSlot::VideoWheel => json!(ring.video_normal_wheel_action),
            },
        }
    }

    fn choices(self) -> Vec<OperationChoice> {
        let actions = match self {
            Self::Ring(context, _) => Some((RingActionId::available_for_context(context), context)),
            Self::Mouse(context, _) => Some((
                RingActionId::available_for_mouse_button_context(context),
                context,
            )),
            Self::Gesture(context) => Some((
                RingActionId::available_for_right_drag_context(context),
                context.gesture_action_context(),
            )),
            _ => None,
        };
        if let Some((actions, context)) = actions {
            return actions
                .into_iter()
                .map(|action| OperationChoice {
                    value: json!(action),
                    label: action.label_for_context(context).into(),
                })
                .collect();
        }
        match self {
            Self::RightDrag(context) => [
                RightDragMode::Disabled,
                RightDragMode::RingShortcut,
                RightDragMode::MouseGesture,
            ]
            .into_iter()
            .filter(|mode| {
                context != RightDragContext::EditMode || *mode != RightDragMode::RingShortcut
            })
            .map(|mode| OperationChoice {
                value: json!(mode),
                label: mode.label().into(),
            })
            .collect(),
            Self::ShortClick(_) => [
                ViewerShortRightClickAction::CloseFullscreen,
                ViewerShortRightClickAction::None,
                ViewerShortRightClickAction::ContextMenu,
            ]
            .into_iter()
            .map(|mode| OperationChoice {
                value: json!(mode),
                label: mode.label().into(),
            })
            .collect(),
            Self::Option(OptionSlot::VideoWheel) => VideoNormalWheelActionId::available()
                .iter()
                .map(|mode| OperationChoice {
                    value: json!(mode),
                    label: mode.label().into(),
                })
                .collect(),
            Self::Option(_) => [false, true]
                .into_iter()
                .map(|value| OperationChoice {
                    value: json!(value),
                    label: if value { "ON" } else { "OFF" }.into(),
                })
                .collect(),
            _ => unreachable!(),
        }
    }

    fn value_label(self, value: &Value) -> Value {
        let choices = self.choices();
        let label = |value: &Value| {
            choices
                .iter()
                .find(|choice| &choice.value == value)
                .map(|choice| choice.label.clone())
                .unwrap_or_else(|| value.to_string())
        };
        if let Self::Gesture(_) = self {
            json!(
                value
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|row| {
                        format!(
                            "{}: {}",
                            row["pattern"].as_str().unwrap_or(""),
                            label(&row["action"])
                        )
                    })
                    .collect::<Vec<_>>()
            )
        } else {
            json!(label(value))
        }
    }

    fn assign(
        self,
        bundle: &mut OperationCustomizeBundle,
        value: &Value,
    ) -> Result<(), AgentError> {
        let name = self.name();
        let invalid = || {
            bad(format!(
                "{name}: 値または操作がこのスロットに合いません。choices を確かめてください"
            ))
        };
        let ring = &mut bundle.ring_shortcuts;
        match self {
            Self::Ring(context, direction) => {
                let action = ring_action(value).ok_or_else(invalid)?;
                if !action.is_valid_for_context(context) {
                    return Err(invalid());
                }
                let target = ring
                    .profile_mut(context)
                    .slots
                    .get_mut(direction.slot_index())
                    .ok_or_else(invalid)?;
                *target = action;
            }
            Self::Mouse(context, button) => {
                let action = ring_action(value).ok_or_else(invalid)?;
                if !action.is_available_for_mouse_button_assignment(context) {
                    return Err(invalid());
                }
                let profile = ring.mouse_button_profile_mut(context);
                *match button {
                    MouseButtonSlot::Back => &mut profile.back,
                    MouseButtonSlot::Forward => &mut profile.forward,
                    MouseButtonSlot::Middle => &mut profile.middle,
                } = action;
            }
            Self::Gesture(context) => {
                let entries = value.as_array().ok_or_else(invalid)?;
                let mut bindings = Vec::new();
                for entry in entries {
                    let object = entry.as_object().ok_or_else(invalid)?;
                    if object.len() != 2 {
                        return Err(invalid());
                    }
                    let pattern = object
                        .get("pattern")
                        .and_then(Value::as_str)
                        .and_then(parse_pattern)
                        .ok_or_else(invalid)?;
                    let action = object
                        .get("action")
                        .and_then(ring_action)
                        .ok_or_else(invalid)?;
                    if !action.is_valid_for_right_drag_context(context) {
                        return Err(invalid());
                    }
                    bindings.push(MouseGestureBinding::new(pattern, action));
                }
                ring.mouse_gesture_profile_mut(context).bindings = bindings;
            }
            Self::RightDrag(context) => {
                let text = value.as_str().ok_or_else(invalid)?;
                let mode = RightDragMode::from_str(text)
                    .filter(|mode| mode.as_str() == text)
                    .ok_or_else(invalid)?;
                if context == RightDragContext::EditMode && mode == RightDragMode::RingShortcut {
                    return Err(invalid());
                }
                ring.set_right_drag_mode(context, mode);
            }
            Self::ShortClick(context) => {
                let text = value.as_str().ok_or_else(invalid)?;
                let mode = ViewerShortRightClickAction::from_str(text)
                    .filter(|mode| mode.as_str() == text)
                    .ok_or_else(invalid)?;
                ring.set_viewer_short_right_click_action(context, mode);
            }
            Self::Option(OptionSlot::VideoWheel) => {
                ring.video_normal_wheel_action =
                    serde_json::from_value(value.clone()).map_err(|_| invalid())?;
            }
            Self::Option(option) => {
                let value = value.as_bool().ok_or_else(invalid)?;
                *match option {
                    OptionSlot::SelectGrid => &mut ring.select_grid_item_on_right_drag_start,
                    OptionSlot::RingHelp => &mut ring.mouse_ring_help_visible,
                    OptionSlot::GestureHelp => &mut ring.mouse_gesture_help_visible,
                    OptionSlot::GestureToast => &mut ring.mouse_gesture_result_toast_visible,
                    OptionSlot::Gamepad => &mut bundle.gamepad_enabled,
                    OptionSlot::VideoWheel => unreachable!(),
                } = value;
            }
        }
        Ok(())
    }
}

fn ring_action(value: &Value) -> Option<RingActionId> {
    let text = value.as_str()?;
    RingActionId::from_str(text)
        .filter(|action| action.as_str() == text && !matches!(action, RingActionId::Unknown(_)))
}

fn pattern_name(pattern: &[MouseGestureDirection]) -> String {
    pattern
        .iter()
        .map(|direction| match direction {
            MouseGestureDirection::Up => 'U',
            MouseGestureDirection::Right => 'R',
            MouseGestureDirection::Down => 'D',
            MouseGestureDirection::Left => 'L',
        })
        .collect()
}

fn parse_pattern(text: &str) -> Option<Vec<MouseGestureDirection>> {
    let mut pattern = Vec::new();
    for ch in text.chars() {
        let direction = match ch {
            'U' => MouseGestureDirection::Up,
            'R' => MouseGestureDirection::Right,
            'D' => MouseGestureDirection::Down,
            'L' => MouseGestureDirection::Left,
            _ => return None,
        };
        if pattern.last() == Some(&direction) {
            return None;
        }
        pattern.push(direction);
    }
    if pattern.is_empty() || pattern.len() > MOUSE_GESTURE_MAX_STROKES {
        None
    } else {
        Some(pattern)
    }
}

/// Request-local interpretation of the persisted override rows. Keep every row
/// identity for effective-key parsing and mutation, and the last position for
/// constant-time public-value lookup. Unknown rows retain their original place.
#[derive(Default)]
struct KeyOverrides {
    rows: Vec<Option<KeyAction>>,
    last: HashMap<KeyAction, usize>,
}

impl KeyOverrides {
    fn new(settings: &KeymapSettings) -> Self {
        let mut index = Self::default();
        for binding in &settings.overrides {
            index.push(KeyAction::parse_ini_name(&binding.action));
        }
        index
    }

    fn push(&mut self, action: Option<KeyAction>) {
        if let Some(action) = action {
            self.last.insert(action, self.rows.len());
        }
        self.rows.push(action);
    }

    fn value(&self, settings: &KeymapSettings, action: KeyAction) -> Value {
        self.last
            .get(&action)
            .map(|index| json!(settings.overrides[*index].chords))
            .unwrap_or(Value::Null)
    }

    fn keymap(&self, settings: &KeymapSettings) -> Keymap {
        Keymap::from_resolved_overrides(self.rows.iter().zip(&settings.overrides).filter_map(
            |(action, binding)| action.map(|action| (action, binding.chords.as_slice())),
        ))
    }

    /// Apply the whole validated key edit set in one pass, so removals cannot
    /// stale stored positions. Replace the last row in place; null removes all
    /// aliases for that action. Newly customized actions append in request order.
    fn apply(
        &self,
        settings: &mut KeymapSettings,
        changes: &HashMap<KeyAction, Value>,
        order: &[KeyAction],
    ) -> Self {
        let binding = |action: KeyAction, value: &Value| KeyBindingOverride {
            action: action.ini_name().into(),
            chords: value
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_owned())
                .collect(),
        };
        let mut next = Self::default();
        let mut updated = Vec::with_capacity(settings.overrides.len() + order.len());
        for (position, mut row) in std::mem::take(&mut settings.overrides)
            .into_iter()
            .enumerate()
        {
            let action = self.rows[position];
            if let Some(action) = action
                && let Some(value) = changes.get(&action)
            {
                if value.is_null() {
                    continue;
                }
                if self.last.get(&action) == Some(&position) {
                    row = binding(action, value);
                }
            }
            next.push(action);
            updated.push(row);
        }
        for &action in order {
            let value = &changes[&action];
            if !self.last.contains_key(&action) && !value.is_null() {
                next.push(Some(action));
                updated.push(binding(action, value));
            }
        }
        settings.overrides = updated;
        next
    }
}

fn conflict_row(conflict: BindingConflict) -> OperationConflict {
    OperationConflict {
        key: conflict.chord.settings_name(),
        kind: match conflict.kind {
            BindingConflictKind::Hard => "same_context",
            BindingConflictKind::ActiveOverlap => "overlap",
            BindingConflictKind::TriggerMismatch => "trigger_mismatch",
            BindingConflictKind::Reserved => "reserved",
        }
        .into(),
        action: conflict.action.ini_name().into(),
        other: conflict
            .other_action
            .map(|action| action.ini_name().to_owned())
            .unwrap_or_else(|| {
                conflict
                    .reserved_kind
                    .map(|kind| kind.label())
                    .unwrap_or("固定入力")
                    .into()
            }),
    }
}

fn related(conflict: &BindingConflict, actions: &[KeyAction]) -> bool {
    actions.contains(&conflict.action)
        || conflict
            .other_action
            .is_some_and(|action| actions.contains(&action))
}

pub(crate) fn prepare_edits(
    before: &OperationCustomizeBundle,
    edits: &OperationEdits,
) -> Result<(OperationCustomizeBundle, OperationsWriteResult), AgentError> {
    edits.validate().map_err(bad)?;
    let mut candidate = before.clone();
    let defaults = OperationCustomizeBundle::defaults();
    let mut result = OperationsWriteResult::default();
    let mut changed_actions = Vec::new();
    let before_keys = KeyOverrides::new(&before.keymap);
    let mut key_changes = HashMap::new();
    for (name, value) in edits.keys.iter().flatten() {
        let action = action(name)?;
        let target = if value.is_null() {
            Value::Null
        } else {
            let values = value.as_array().filter(|v| v.len() <= 3).ok_or_else(|| {
                bad(format!(
                    "{name}: キーは 0～3 個の配列、既定に戻すときは null で指定してください"
                ))
            })?;
            let mut chords = Vec::with_capacity(values.len());
            for value in values {
                let text = value
                    .as_str()
                    .ok_or_else(|| bad(format!("{name}: キーは文字列です")))?;
                let chord = keymap::parse_chord_for_action(action, text)
                    .map_err(|error| bad(format!("{name}: {error}")))?
                    .ok_or_else(|| {
                        bad(format!(
                            "{name}: 空と none は使えません。割り当てなしは [] です"
                        ))
                    })?;
                // Preserve order and duplicates, but compare and persist only
                // the canonical settings spelling, as the customization UI does.
                chords.push(Value::String(chord.settings_name()));
            }
            Value::Array(chords)
        };
        let old = before_keys.value(&before.keymap, action);
        let change = OperationChange {
            name: name.clone(),
            label: format!(
                "キー: {} ({})",
                keymap::compact_key_action_label(action),
                action.context().description()
            ),
            before: old,
            after: target,
        };
        if change.before == change.after {
            result.unchanged.push(change);
        } else {
            key_changes.insert(action, change.after.clone());
            changed_actions.push(action);
            result.changed.push(change);
        }
    }
    for (name, value) in edits.inputs.iter().flatten() {
        let slot = Slot::parse(name)?;
        let target = if value.is_null() {
            slot.value(&defaults)
        } else {
            value.clone()
        };
        // Validate even a no-op, without changing the live bundle.
        let mut validated = candidate.clone();
        slot.assign(&mut validated, &target)?;
        let mut normalized = validated.clone();
        normalized.ring_shortcuts.sanitize();
        if slot.value(&validated) != slot.value(&normalized) {
            return Err(bad(format!(
                "{name}: 正規化で値が変わるため受け付けられません"
            )));
        }
        let change = OperationChange {
            name: name.clone(),
            label: slot.label(),
            before: slot.value(before),
            after: target,
        };
        if change.before == change.after {
            result.unchanged.push(change);
        } else {
            candidate = validated;
            result.changed.push(change);
        }
    }
    if !changed_actions.is_empty() {
        let candidate_keys =
            before_keys.apply(&mut candidate.keymap, &key_changes, &changed_actions);
        result.conflicts = candidate_keys
            .keymap(&candidate.keymap)
            .binding_conflicts()
            .into_iter()
            .filter(|conflict| related(conflict, &changed_actions))
            .map(conflict_row)
            .collect();
    }

    Ok((candidate, result))
}

pub(crate) fn readback(
    before: &OperationCustomizeBundle,
    after: &OperationCustomizeBundle,
    prepared: &OperationsWriteResult,
) -> OperationsWriteResult {
    let mut result = prepared.clone();
    let before_keys = KeyOverrides::new(&before.keymap);
    let after_keys = KeyOverrides::new(&after.keymap);
    for row in result.changed.iter_mut().chain(&mut result.unchanged) {
        let key_action = action(&row.name).ok();
        row.before = if let Some(action) = key_action {
            before_keys.value(&before.keymap, action)
        } else {
            Slot::parse(&row.name).unwrap().value(before)
        };
        row.after = if let Some(action) = key_action {
            after_keys.value(&after.keymap, action)
        } else {
            Slot::parse(&row.name).unwrap().value(after)
        };
    }
    result
}

pub(crate) fn change_text(name: &str, value: &Value) -> String {
    if let Ok(slot) = Slot::parse(name) {
        let label = slot.value_label(value);
        if let Some(text) = label.as_str() {
            return text.into();
        }
        return label
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" / ");
    }
    if value.is_null() {
        let labels: Vec<_> = action(name)
            .unwrap()
            .default_chords()
            .iter()
            .map(Chord::settings_name)
            .collect();
        return if labels.is_empty() {
            "なし".into()
        } else {
            labels.join(" / ")
        };
    }
    let keys = value.as_array().unwrap();
    if keys.is_empty() {
        "なし".into()
    } else {
        keys.iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" / ")
    }
}

pub(crate) fn get_operations(
    bundle: &OperationCustomizeBundle,
    request: &OperationsQuery,
) -> Result<OperationsPage, AgentError> {
    let view = request.view.as_deref().unwrap_or("keys");
    request.validate().map_err(bad)?;
    let actions = request
        .actions
        .as_ref()
        .map(|names| {
            names
                .iter()
                .map(|name| action(name))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    let chord = request
        .key
        .as_ref()
        .map(|key| keymap::parse_operation_chord(key).map_err(|error| bad(format!("key: {error}"))))
        .transpose()?;
    let mut reserved = Vec::new();
    let mut note = None;
    let mut rows: Vec<OperationRow> = match view {
        "keys" => {
            let overrides = KeyOverrides::new(&bundle.keymap);
            let keymap = overrides.keymap(&bundle.keymap);
            if let Some(context) = &request.context
                && !KeyAction::all()
                    .iter()
                    .any(|action| action.context().ini_name() == context)
            {
                return Err(bad(format!("{context}: 不明な context です")));
            }
            if let Some(screen) = &request.screen
                && !keymap::OPERATION_SCREENS
                    .iter()
                    .any(|entry| entry.name == screen)
            {
                return Err(bad(format!("{screen}: 不明な screen です")));
            }
            if let Some(chord) = chord {
                reserved = keymap::reserved_labels_for_screen(chord, request.screen.as_deref());
            }
            let terms: Vec<_> = request
                .query
                .as_deref()
                .unwrap_or("")
                .split_whitespace()
                .map(str::to_lowercase)
                .collect();
            let selected: Vec<_> = KeyAction::all()
                .iter()
                .copied()
                .filter(|action| {
                    action.is_user_facing()
                        && actions
                            .as_ref()
                            .is_none_or(|actions| actions.contains(action))
                        && chord
                            .is_none_or(|chord| keymap.effective_chords(*action).contains(&chord))
                        && request
                            .context
                            .as_ref()
                            .is_none_or(|context| action.context().ini_name() == context)
                        && request
                            .screen
                            .as_ref()
                            .is_none_or(|screen| keymap::action_active_on_screen(*action, screen))
                        && terms.iter().all(|term| {
                            format!(
                                "{} {} {}",
                                action.ini_name(),
                                keymap::compact_key_action_label(*action),
                                action.context().description()
                            )
                            .to_lowercase()
                            .contains(term)
                        })
                })
                .collect();
            if selected
                .iter()
                .map(|action| action.context())
                .collect::<HashSet<_>>()
                .len()
                > 1
            {
                note = Some(
                    "操作は画面ごとに別です。どの画面のつもりかを利用者に確かめてください".into(),
                );
            }
            selected
                .into_iter()
                .map(|action| {
                    let bindings = ring_bindings_for_key_action(action);
                    let inputs = Slot::all()
                        .into_iter()
                        .filter(|slot| match *slot {
                            Slot::Ring(context, direction) => bindings.iter().any(|(c, a)| {
                                *c == context
                                    && bundle
                                        .ring_shortcuts
                                        .profile(context)
                                        .slots
                                        .get(direction.slot_index())
                                        == Some(a)
                            }),
                            Slot::Mouse(context, button) => bindings.iter().any(|(c, a)| {
                                *c == context
                                    && bundle
                                        .ring_shortcuts
                                        .mouse_button_profile(context)
                                        .action(button)
                                        == *a
                            }),
                            Slot::Gesture(context) => bindings.iter().any(|(c, a)| {
                                *c == context.gesture_action_context()
                                    && bundle
                                        .ring_shortcuts
                                        .mouse_gesture_profile(context)
                                        .bindings
                                        .iter()
                                        .any(|binding| &binding.action == a)
                            }),
                            _ => false,
                        })
                        .map(Slot::name)
                        .collect();
                    let screens: Vec<_> = keymap::OPERATION_SCREENS
                        .iter()
                        .filter(|screen| keymap::action_active_on_screen(action, screen.name))
                        .collect();
                    OperationRow::Key(OperationKeyRow {
                        action: action.ini_name().into(),
                        label: keymap::compact_key_action_label(action),
                        context: action.context().ini_name().into(),
                        context_label: action.context().description().into(),
                        trigger: match action.trigger() {
                            KeyTrigger::Press => "press",
                            KeyTrigger::ModifierHold => "modifier_hold",
                            KeyTrigger::KeyHold => "key_hold",
                        }
                        .into(),
                        keys: keymap
                            .effective_chords(action)
                            .into_iter()
                            .map(Chord::settings_name)
                            .collect(),
                        default: action
                            .default_chords()
                            .iter()
                            .map(Chord::settings_name)
                            .collect(),
                        customized: overrides.last.contains_key(&action),
                        inputs,
                        screens: screens
                            .iter()
                            .map(|screen| screen.name.to_owned())
                            .collect(),
                        screen_labels: screens
                            .iter()
                            .map(|screen| screen.label.to_owned())
                            .collect(),
                    })
                })
                .collect()
        }
        "inputs" => {
            let defaults = OperationCustomizeBundle::defaults();
            Slot::all()
                .into_iter()
                .filter(|slot| {
                    request
                        .slots
                        .as_ref()
                        .is_none_or(|prefix| slot.name().starts_with(prefix))
                })
                .map(|slot| {
                    let value = slot.value(bundle);
                    OperationRow::Input(OperationInputRow {
                        slot: slot.name(),
                        label: slot.label(),
                        value_label: slot.value_label(&value),
                        value,
                        default: slot.value(&defaults),
                    })
                })
                .collect()
        }
        "choices" => Slot::parse(
            request
                .slot
                .as_deref()
                .ok_or_else(|| bad("choices には slot を 1 つ指定してください"))?,
        )?
        .choices()
        .into_iter()
        .map(OperationRow::Choice)
        .collect(),
        "conflicts" => {
            let mut candidate = bundle.clone();
            let mut overrides = KeyOverrides::new(&candidate.keymap);
            if let (Some(actions), Some(key)) = (&actions, &request.key) {
                let action = actions[0];
                let chord = keymap::parse_chord_for_action(action, key)
                    .map_err(|error| bad(format!("{}: {error}", action.ini_name())))?
                    .ok_or_else(|| bad("key に空と none は使えません"))?;
                let changes = HashMap::from([(action, json!([chord.settings_name()]))]);
                overrides = overrides.apply(&mut candidate.keymap, &changes, &[action]);
            }
            overrides
                .keymap(&candidate.keymap)
                .binding_conflicts()
                .into_iter()
                .filter(|conflict| {
                    actions
                        .as_ref()
                        .is_none_or(|actions| related(conflict, actions))
                        && chord.is_none_or(|chord| chord == conflict.chord)
                })
                .map(conflict_row)
                .map(OperationRow::Conflict)
                .collect()
        }
        _ => unreachable!(),
    };
    let page = request.page.unwrap_or(1);
    let pages = rows.len().div_ceil(SETTINGS_PAGE_SIZE).max(1) as u32;
    if page == 0 || page > pages {
        return Err(bad("指定したページはありません。page は 1 からです"));
    }
    rows = rows
        .into_iter()
        .skip((page as usize - 1) * SETTINGS_PAGE_SIZE)
        .take(SETTINGS_PAGE_SIZE)
        .collect();
    let mut response = OperationsPage {
        items: rows.into_iter().map(PageRow::Value).collect(),
        page,
        pages,
        reserved,
        note,
    };
    let items = std::mem::take(&mut response.items)
        .into_iter()
        .map(|row| match row {
            PageRow::Value(row) => row,
            PageRow::TooLong(_) => unreachable!(),
        })
        .collect();
    response.items = bound_page_rows(
        items,
        |row| match row {
            OperationRow::Key(row) => row.action.clone(),
            OperationRow::Input(row) => row.slot.clone(),
            OperationRow::Conflict(row) => {
                format!("{}:{}:{}:{}", row.key, row.kind, row.action, row.other)
            }
            OperationRow::Choice(row) => row.value.to_string(),
        },
        |items| OperationsPage {
            items: items.to_vec(),
            ..response.clone()
        },
    )
    .map_err(bad)?;
    Ok(response)
}

#[cfg(test)]
mod tests;
