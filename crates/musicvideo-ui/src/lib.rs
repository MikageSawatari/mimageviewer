//! ラボと本体で共有する音楽ビデオ編集 UI。
//!
//! ウィンドウ、ファイル I/O、音声再生、書き出しは呼び出し側が所有する。

use std::path::PathBuf;

mod document;
mod history;
mod keyframes;
mod modulators;
mod resources;
mod timeline;

pub use document::{DocumentSession, DocumentSessionId, EditGeneration, SaveCheckpoint};
pub use keyframes::{
    AnimProperty, AnimationValue, CommandResult, EditorCommand, EditorCommandError,
    LayerMoveDirection, apply_editor_command,
};
pub use resources::{EditTargetLifetime, ResourceLease, ResourceManifest, ResourceRevision};
pub use timeline::{TimelineResponse, TimelineWaveform, draw_timeline};

use egui::TextureHandle;
use musicvideo_core::{
    Fill, Glow, GradientStop, GradientStops, ImageId, Layer, LayerId, LayerKind, LyricsLayer,
    MIV_SPECTRUM_BANDS, ModulatorCalibration, ModulatorValue, MusicColorMode, Project, Rgba8,
    SpectrumDirection, SpectrumLayer, SpectrumShape, TextAlign, TextLayer, TimeFormat,
    TimeTextLayer, new_linear_spectrum_layer, new_lyrics_layer, new_miv_spectrum_keyboard_layers,
    new_miv_spectrum_layer, new_pitch_keyboard_layer, new_progress_bar_layer,
    new_radial_spectrum_layer, new_solid_layer, new_text_layer, new_time_text_layer,
};

use history::{EditSurface, UndoHistory};
use modulators::ModulatorEditorState;
use resources::EditTargetLifetimes;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailSelection {
    Key {
        property: AnimProperty,
        key_index: usize,
    },
    Lyric {
        line_index: usize,
    },
}

#[derive(Debug, Clone, PartialEq)]
struct EditorSnapshot {
    project: Project,
    resources: ResourceManifest,
    selected_layer: Option<LayerId>,
    selected_detail: Option<DetailSelection>,
}

/// Opaque pre-edit state used when a host-owned operation changes the project.
///
/// File dialogs and decoding remain outside this crate, but committing their
/// project mutation through this token keeps Undo available when the shared UI
/// is embedded in another application.
#[derive(Debug, Clone, PartialEq)]
pub struct EditCheckpoint(EditorSnapshot);

#[derive(Debug, Clone, PartialEq)]
pub struct EditorState {
    selected_layer: Option<LayerId>,
    selected_detail: Option<DetailSelection>,
    dragging_detail: Option<DetailSelection>,
    resources: ResourceManifest,
    target_lifetimes: EditTargetLifetimes,
    lyrics_notice: Option<(LayerId, String)>,
    pending_layer_delete: Option<LayerId>,
    modulator_editor: ModulatorEditorState,
    history: UndoHistory,
}

impl Default for EditorState {
    fn default() -> Self {
        Self {
            selected_layer: None,
            selected_detail: None,
            dragging_detail: None,
            resources: ResourceManifest::default(),
            target_lifetimes: EditTargetLifetimes::default(),
            lyrics_notice: None,
            pending_layer_delete: None,
            modulator_editor: ModulatorEditorState::default(),
            history: UndoHistory::default(),
        }
    }
}

impl EditorState {
    pub fn selected_layer(&self, project: &Project) -> usize {
        self.selected_layer
            .and_then(|id| project.layers.iter().position(|layer| layer.id == id))
            .unwrap_or_else(|| usize::from(project.layers.len() > 1))
    }

    pub fn select_layer(&mut self, project: &Project, index: usize) {
        let next = project.layers.get(index).map(|layer| layer.id);
        if self.selected_layer != next {
            self.clear_detail_selection();
        }
        self.selected_layer = next;
    }

    /// Clears selections and pending confirmations that belong to the previous project.
    pub fn reset_for_project(&mut self, project: &Project) {
        self.reset_for_project_with_resources(project, ResourceManifest::default());
    }

    pub fn reset_for_project_with_resources(
        &mut self,
        project: &Project,
        mut resources: ResourceManifest,
    ) {
        resources.retain_project_resources(project);
        self.resources = resources;
        self.target_lifetimes.reset(project);
        self.selected_layer = project
            .layers
            .get(1)
            .or_else(|| project.layers.first())
            .map(|layer| layer.id);
        self.clear_detail_selection();
        self.lyrics_notice = None;
        self.pending_layer_delete = None;
        self.modulator_editor = ModulatorEditorState::default();
        self.history.clear();
    }

    pub fn reconcile(&mut self, project: &Project) {
        self.target_lifetimes.reconcile(project);
        if self
            .selected_layer
            .is_none_or(|selected| !project.layers.iter().any(|layer| layer.id == selected))
        {
            self.selected_layer = project
                .layers
                .get(1)
                .or_else(|| project.layers.first())
                .map(|layer| layer.id);
            self.clear_detail_selection();
        }
        let active_layer = self
            .selected_layer
            .and_then(|id| project.layers.iter().find(|layer| layer.id == id));
        let detail_exists = match (self.selected_detail, active_layer) {
            (
                Some(DetailSelection::Key {
                    property,
                    key_index,
                }),
                Some(layer),
            ) => keyframes::animation_keys(layer, property)
                .is_some_and(|keys| key_index < keys.len()),
            (Some(DetailSelection::Lyric { line_index }), Some(layer)) => {
                matches!(&layer.kind, LayerKind::Lyrics(lyrics) if line_index < lyrics.lines.len())
            }
            (None, _) => true,
            (Some(_), None) => false,
        };
        if !detail_exists {
            self.clear_detail_selection();
        }
        if self
            .pending_layer_delete
            .is_some_and(|id| !project.layers.iter().any(|layer| layer.id == id))
        {
            self.pending_layer_delete = None;
        }
    }

    fn select_layer_id(&mut self, project: &Project, id: LayerId) {
        if project.layers.iter().any(|layer| layer.id == id) {
            if self.selected_layer != Some(id) {
                self.clear_detail_selection();
            }
            self.selected_layer = Some(id);
        }
    }

    fn selected_key(&self) -> Option<(AnimProperty, usize)> {
        match self.selected_detail {
            Some(DetailSelection::Key {
                property,
                key_index,
            }) => Some((property, key_index)),
            _ => None,
        }
    }

    fn selected_lyric(&self) -> Option<usize> {
        match self.selected_detail {
            Some(DetailSelection::Lyric { line_index }) => Some(line_index),
            _ => None,
        }
    }

    fn clear_detail_selection(&mut self) {
        self.selected_detail = None;
        self.dragging_detail = None;
    }

    fn select_key(
        &mut self,
        project: &Project,
        layer: LayerId,
        property: AnimProperty,
        key_index: usize,
    ) {
        self.select_layer_id(project, layer);
        self.selected_detail = Some(DetailSelection::Key {
            property,
            key_index,
        });
    }

    fn select_lyric(&mut self, project: &Project, layer: LayerId, line_index: usize) {
        self.select_layer_id(project, layer);
        self.selected_detail = Some(DetailSelection::Lyric { line_index });
    }

    fn snapshot(&self, project: &Project) -> EditorSnapshot {
        EditorSnapshot {
            project: project.clone(),
            resources: self.resources.clone(),
            selected_layer: self.selected_layer,
            selected_detail: self.selected_detail,
        }
    }

    fn restore_snapshot(&mut self, project: &mut Project, snapshot: EditorSnapshot) {
        *project = snapshot.project;
        self.resources = snapshot.resources;
        self.selected_layer = snapshot
            .selected_layer
            .filter(|id| project.layers.iter().any(|layer| layer.id == *id))
            .or_else(|| {
                project
                    .layers
                    .get(1)
                    .or_else(|| project.layers.first())
                    .map(|layer| layer.id)
            });
        self.selected_detail = snapshot.selected_detail;
        self.dragging_detail = None;
        self.pending_layer_delete = None;
        self.modulator_editor = ModulatorEditorState::default();
        self.reconcile(project);
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub fn history_depths(&self) -> (usize, usize) {
        (self.history.undo_len(), self.history.redo_len())
    }

    pub fn undo(&mut self, project: &mut Project) -> bool {
        let current = self.snapshot(project);
        let Some(snapshot) = self.history.undo(current) else {
            return false;
        };
        self.restore_snapshot(project, snapshot);
        true
    }

    pub fn redo(&mut self, project: &mut Project) -> bool {
        let current = self.snapshot(project);
        let Some(snapshot) = self.history.redo(current) else {
            return false;
        };
        self.restore_snapshot(project, snapshot);
        true
    }

    pub fn begin_external_edit(&self, project: &Project) -> EditCheckpoint {
        EditCheckpoint(self.snapshot(project))
    }

    pub fn finish_external_edit(&mut self, project: &Project, checkpoint: EditCheckpoint) -> bool {
        self.target_lifetimes.reconcile(project);
        self.resources.retain_project_resources(project);
        let after = self.snapshot(project);
        let changed = checkpoint.0 != after;
        self.history.record_atomic(checkpoint.0, &after);
        changed
    }

    pub fn finish_external_edit_with_resources(
        &mut self,
        project: &Project,
        checkpoint: EditCheckpoint,
        mut resources: ResourceManifest,
    ) -> bool {
        self.target_lifetimes.reconcile(project);
        resources.retain_project_resources(project);
        self.resources = resources;
        let after = self.snapshot(project);
        let changed = checkpoint.0 != after;
        self.history.record_atomic(checkpoint.0, &after);
        changed
    }

    pub fn resource_manifest(&self) -> &ResourceManifest {
        &self.resources
    }

    pub fn layer_target_lifetime(&self, id: LayerId) -> Option<EditTargetLifetime> {
        self.target_lifetimes.layer(id)
    }

    pub fn image_target_lifetime(&self, id: &ImageId) -> Option<EditTargetLifetime> {
        self.target_lifetimes.image(id)
    }

    pub fn apply_external_command(
        &mut self,
        project: &mut Project,
        command: &EditorCommand,
    ) -> bool {
        self.try_apply_external_command(project, command)
            .unwrap_or(false)
    }

    pub fn try_apply_external_command(
        &mut self,
        project: &mut Project,
        command: &EditorCommand,
    ) -> Result<bool, EditorCommandError> {
        let checkpoint = self.begin_external_edit(project);
        let changed = self.try_apply_command(project, command)?;
        if changed {
            self.finish_external_edit(project, checkpoint);
        }
        Ok(changed)
    }

    fn observe_ui_pass(
        &mut self,
        surface: EditSurface,
        project: &Project,
        checkpoint: EditCheckpoint,
        changed: bool,
        pointer_active: bool,
        text_focus: Option<egui::Id>,
    ) {
        self.resources.retain_project_resources(project);
        let after = self.snapshot(project);
        self.history.observe_ui_pass(
            surface,
            checkpoint.0,
            &after,
            changed,
            pointer_active,
            text_focus,
        );
    }

    fn apply_command(&mut self, project: &mut Project, command: &EditorCommand) -> bool {
        self.try_apply_command(project, command).unwrap_or(false)
    }

    fn try_apply_command(
        &mut self,
        project: &mut Project,
        command: &EditorCommand,
    ) -> Result<bool, EditorCommandError> {
        let selected_layer = project
            .layers
            .get(self.selected_layer(project))
            .map(|layer| layer.id);
        let animation_target = command.animation_target();
        let result = apply_editor_command(project, command)?;
        self.resources.retain_project_resources(project);
        if let Some((layer, property)) = animation_target {
            self.select_layer_id(project, layer);
            if result.clear_selection
                && self
                    .selected_key()
                    .is_some_and(|(selected_property, _)| selected_property == property)
            {
                self.clear_detail_selection();
            } else if let Some(key_index) = result.selected_key {
                let selection = DetailSelection::Key {
                    property,
                    key_index,
                };
                self.selected_detail = Some(selection);
                if matches!(
                    self.dragging_detail,
                    Some(DetailSelection::Key {
                        property: dragging_property,
                        ..
                    }) if dragging_property == property
                ) {
                    self.dragging_detail = Some(selection);
                }
            }
        } else {
            if let Some(selected_layer) = selected_layer {
                self.select_layer_id(project, selected_layer);
            }
            self.reconcile(project);
        }
        if matches!(
            command,
            EditorCommand::ReplaceLyrics { .. } | EditorCommand::ReplaceLyricsText { .. }
        ) && matches!(self.selected_detail, Some(DetailSelection::Lyric { .. }))
        {
            self.clear_detail_selection();
        }
        Ok(result.changed)
    }

    pub fn set_lyrics_notice(&mut self, layer: LayerId, notice: String) {
        self.lyrics_notice = Some((layer, notice));
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EditorResponse {
    pub project_changed: bool,
    pub history_changed: bool,
    pub add_image_layer_requested: bool,
    pub import_lrc_requested: Option<LayerId>,
    pub export_lrc_requested: Option<LayerId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NewLayerPreset {
    Image,
    RadialSpectrum,
    LinearSpectrum,
    MivSpectrum,
    MivSpectrumKeyboard,
    PitchKeyboard,
    ProgressBar,
    TimeText,
    Text,
    Lyrics,
    Solid,
}

const NEW_LAYER_PRESETS: [(NewLayerPreset, &str); 11] = [
    (NewLayerPreset::Image, "画像"),
    (NewLayerPreset::RadialSpectrum, "円形スペクトラム"),
    (NewLayerPreset::LinearSpectrum, "直線スペクトラム"),
    (NewLayerPreset::MivSpectrum, "mIV と同じ横長スペクトラム"),
    (
        NewLayerPreset::MivSpectrumKeyboard,
        "mIV スペクトラム + 鍵盤",
    ),
    (NewLayerPreset::PitchKeyboard, "ピッチ鍵盤"),
    (NewLayerPreset::ProgressBar, "進捗バー"),
    (NewLayerPreset::TimeText, "時間表示"),
    (NewLayerPreset::Text, "テキスト"),
    (NewLayerPreset::Lyrics, "歌詞"),
    (NewLayerPreset::Solid, "単色"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingImageNotice {
    pub id: ImageId,
    pub layer_name: String,
    pub path: Option<PathBuf>,
    pub reason: String,
}

/// Draws the layer list and property editor without owning the project.
pub fn draw_editor(
    ui: &mut egui::Ui,
    project: &mut Project,
    state: &mut EditorState,
    current_time_secs: f64,
    modulator_values: &[ModulatorValue],
    calibrations: &[ModulatorCalibration],
) -> EditorResponse {
    state.reconcile(project);
    let context = ui.ctx().clone();
    let mut history_changed = apply_history_shortcut(&context, project, state);
    ui.horizontal(|ui| {
        ui.heading("プロパティ");
        if ui
            .add_enabled(state.can_undo(), egui::Button::new("元に戻す"))
            .on_hover_text("Ctrl+Z")
            .clicked()
        {
            history_changed |= state.undo(project);
        }
        if ui
            .add_enabled(state.can_redo(), egui::Button::new("やり直す"))
            .on_hover_text("Ctrl+Y / Ctrl+Shift+Z")
            .clicked()
        {
            history_changed |= state.redo(project);
        }
    });
    ui.separator();
    let checkpoint = state.begin_external_edit(project);
    let body_width = ui.available_width();
    let body_height = ui.available_height().max(0.0);
    let allocation = ui.allocate_ui_with_layout(
        egui::vec2(body_width, body_height),
        egui::Layout::top_down(egui::Align::LEFT),
        |ui| {
            ui.set_min_width(body_width);
            ui.set_max_width(body_width);
            ui.set_min_height(body_height);
            ui.style_mut().spacing.scroll = egui::style::ScrollStyle::solid();
            let mut response = egui::ScrollArea::vertical()
                .max_height(body_height)
                .min_scrolled_height(0.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let mut project_changed = ui
                        .add(
                            egui::DragValue::new(&mut project.display_gain_db)
                                .range(-48.0..=48.0)
                                .speed(0.1)
                                .prefix("表示ゲイン ")
                                .suffix(" dB"),
                        )
                        .on_hover_text("鍵盤の点灯強度だけを調整します。音声の音量は変わりません")
                        .changed();
                    project_changed |= modulators::draw_modulators(
                        ui,
                        project,
                        &mut state.modulator_editor,
                        modulator_values,
                        calibrations,
                    );
                    ui.separator();
                    ui.heading("レイヤー");
                    ui.small("一覧の上が奥、下が手前です");
                    for (index, layer) in project.layers.iter().enumerate() {
                        if ui
                            .selectable_label(state.selected_layer == Some(layer.id), &layer.name)
                            .clicked()
                        {
                            state.select_layer(project, index);
                        }
                    }

                    let selected_index = state.selected_layer(project);
                    let selected_layer = project.layers.get(selected_index).map(|layer| layer.id);
                    let mut order_command = None;
                    ui.horizontal(|ui| {
                        let can_move_toward_back = selected_layer.is_some() && selected_index > 0;
                        if ui
                            .add_enabled(
                                can_move_toward_back,
                                egui::Button::new("一覧の上へ（奥）"),
                            )
                            .on_hover_text("描画順を1つ奥へ移動します")
                            .clicked()
                            && let Some(layer) = selected_layer
                        {
                            order_command = Some(EditorCommand::MoveLayer {
                                layer,
                                direction: LayerMoveDirection::TowardBack,
                            });
                        }
                        let can_move_toward_front =
                            selected_layer.is_some() && selected_index + 1 < project.layers.len();
                        if ui
                            .add_enabled(
                                can_move_toward_front,
                                egui::Button::new("一覧の下へ（手前）"),
                            )
                            .on_hover_text("描画順を1つ手前へ移動します")
                            .clicked()
                            && let Some(layer) = selected_layer
                        {
                            order_command = Some(EditorCommand::MoveLayer {
                                layer,
                                direction: LayerMoveDirection::TowardFront,
                            });
                        }
                    });
                    if let Some(command) = order_command {
                        project_changed |= state.apply_command(project, &command);
                    }
                    if ui
                        .add_enabled(
                            selected_layer.is_some(),
                            egui::Button::new("選択中のレイヤーを削除…"),
                        )
                        .clicked()
                    {
                        state.pending_layer_delete = selected_layer;
                    }

                    let mut new_layer = None;
                    let mut add_image_layer_requested = false;
                    ui.menu_button("＋ レイヤーを追加", |ui| {
                        for (preset, label) in NEW_LAYER_PRESETS {
                            if ui.button(label).clicked() {
                                if preset == NewLayerPreset::Image {
                                    add_image_layer_requested = true;
                                } else {
                                    new_layer = Some(preset);
                                }
                                ui.close();
                            }
                        }
                    });
                    if let Some(preset) = new_layer {
                        let layers = match preset {
                            NewLayerPreset::Image => {
                                unreachable!("画像の選択は呼び出し側が所有します")
                            }
                            NewLayerPreset::RadialSpectrum => {
                                vec![new_radial_spectrum_layer(project)]
                            }
                            NewLayerPreset::LinearSpectrum => {
                                vec![new_linear_spectrum_layer(project)]
                            }
                            NewLayerPreset::MivSpectrum => {
                                vec![new_miv_spectrum_layer(project)]
                            }
                            NewLayerPreset::MivSpectrumKeyboard => {
                                new_miv_spectrum_keyboard_layers(project).into()
                            }
                            NewLayerPreset::PitchKeyboard => {
                                vec![new_pitch_keyboard_layer(project)]
                            }
                            NewLayerPreset::ProgressBar => {
                                vec![new_progress_bar_layer(project)]
                            }
                            NewLayerPreset::TimeText => vec![new_time_text_layer(project)],
                            NewLayerPreset::Text => vec![new_text_layer(project)],
                            NewLayerPreset::Lyrics => vec![new_lyrics_layer(project)],
                            NewLayerPreset::Solid => vec![new_solid_layer(project)],
                        };
                        project.layers.extend(layers);
                        state.select_layer(project, project.layers.len() - 1);
                        project_changed = true;
                    }
                    ui.separator();
                    let mut commands = Vec::new();
                    let mut import_lrc_requested = None;
                    let mut export_lrc_requested = None;
                    let output = project.output;
                    let modulators = project.modulators.clone();
                    let selected_index = state.selected_layer(project);
                    if let Some(layer) = project.layers.get_mut(selected_index) {
                        let result = edit_layer_properties(
                            ui,
                            layer,
                            state,
                            current_time_secs,
                            output.width,
                            output.height,
                            &modulators,
                            modulator_values,
                        );
                        project_changed |= result.changed;
                        commands = result.commands;
                        import_lrc_requested = result.import_lrc_requested;
                        export_lrc_requested = result.export_lrc_requested;
                    }
                    for command in commands {
                        project_changed |= state.apply_command(project, &command);
                    }
                    EditorResponse {
                        project_changed,
                        history_changed: false,
                        add_image_layer_requested,
                        import_lrc_requested,
                        export_lrc_requested,
                    }
                })
                .inner;
            if let Some(command) = draw_layer_delete_confirmation(&context, project, state) {
                response.project_changed |= state.apply_command(project, &command);
            }
            response
        },
    );
    let mut response = allocation.inner;
    response.history_changed = history_changed;
    let pointer_active = context.input(|input| input.pointer.primary_down());
    // Context::wants_keyboard_input is true while a TextEdit (including the
    // editable part of DragValue) owns focus. This both defines the text-edit
    // transaction boundary and keeps IME input owned by egui.
    let text_focus = context.memory(|memory| memory.focused());
    state.observe_ui_pass(
        EditSurface::Properties,
        project,
        checkpoint,
        response.project_changed,
        pointer_active,
        text_focus,
    );
    response.project_changed |= history_changed;
    response
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HistoryAction {
    Undo,
    Redo,
}

fn history_action_for_event(
    event: &egui::Event,
    text_input_focused: bool,
) -> Option<HistoryAction> {
    if text_input_focused {
        // TextEdit and the platform IME own Ctrl+Z/Y while the field has focus.
        return None;
    }
    let egui::Event::Key {
        key,
        pressed: true,
        repeat: false,
        modifiers,
        ..
    } = event
    else {
        return None;
    };
    if *key == egui::Key::Z
        && modifiers.matches_exact(egui::Modifiers::CTRL | egui::Modifiers::SHIFT)
    {
        Some(HistoryAction::Redo)
    } else if *key == egui::Key::Z && modifiers.matches_exact(egui::Modifiers::CTRL) {
        Some(HistoryAction::Undo)
    } else if *key == egui::Key::Y && modifiers.matches_exact(egui::Modifiers::CTRL) {
        Some(HistoryAction::Redo)
    } else {
        None
    }
}

fn apply_history_shortcut(
    context: &egui::Context,
    project: &mut Project,
    state: &mut EditorState,
) -> bool {
    let text_input_focused = context.wants_keyboard_input();
    let requested = context.input(|input| {
        input
            .events
            .iter()
            .find_map(|event| history_action_for_event(event, text_input_focused))
    });
    let Some(action) = requested else {
        return false;
    };
    let (modifiers, key) = match action {
        HistoryAction::Undo => (egui::Modifiers::CTRL, egui::Key::Z),
        HistoryAction::Redo => {
            let shift_z = context.input(|input| {
                input.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Key {
                            key: egui::Key::Z,
                            pressed: true,
                            modifiers,
                            ..
                        } if modifiers.matches_exact(egui::Modifiers::CTRL | egui::Modifiers::SHIFT)
                    )
                })
            });
            if shift_z {
                (egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::Z)
            } else {
                (egui::Modifiers::CTRL, egui::Key::Y)
            }
        }
    };
    context.input_mut(|input| {
        input.consume_key(modifiers, key);
    });
    match action {
        HistoryAction::Undo => state.undo(project),
        HistoryAction::Redo => state.redo(project),
    }
}

fn draw_layer_delete_confirmation(
    context: &egui::Context,
    project: &Project,
    state: &mut EditorState,
) -> Option<EditorCommand> {
    let layer_id = state.pending_layer_delete?;
    let Some(layer) = project.layers.iter().find(|layer| layer.id == layer_id) else {
        state.pending_layer_delete = None;
        return None;
    };
    let layer_name = layer.name.clone();
    let mut confirmed = false;
    let mut cancelled = false;
    let response = egui::Modal::new(egui::Id::new("musicvideo-delete-layer-confirmation")).show(
        context,
        |ui| {
            ui.set_min_width(320.0);
            ui.heading("レイヤーの削除");
            ui.add_space(8.0);
            ui.label(format!("レイヤー「{layer_name}」を削除しますか？"));
            ui.small("削除後も「元に戻す」で復元できます。");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("削除する").clicked() {
                    confirmed = true;
                }
                if ui.button("キャンセル").clicked() {
                    cancelled = true;
                }
            });
        },
    );
    if confirmed {
        state.pending_layer_delete = None;
        Some(EditorCommand::DeleteLayer { layer: layer_id })
    } else {
        if cancelled || response.should_close() {
            state.pending_layer_delete = None;
        }
        None
    }
}

/// Draws the already-composited frame. Texture creation and updates stay with the caller.
pub fn draw_preview(ui: &mut egui::Ui, texture: Option<&TextureHandle>, _state: &mut EditorState) {
    if let Some(texture) = texture {
        let source = texture.size_vec2();
        let available = ui.available_size();
        let scale = (available.x / source.x)
            .min(available.y / source.y)
            .max(0.01);
        let size = source * scale;
        ui.centered_and_justified(|ui| {
            ui.image((texture.id(), size));
        });
    } else {
        ui.centered_and_justified(|ui| {
            ui.spinner();
            ui.label("プレビューを準備しています");
        });
    }
}

/// Shows unresolved image details and reports a requested replacement by image ID.
pub fn draw_missing_images(ui: &mut egui::Ui, notices: &[MissingImageNotice]) -> Option<ImageId> {
    let mut replacement = None;
    for notice in notices {
        let path = notice
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "パス未指定".to_owned());
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                format!(
                    "未解決素材: レイヤー「{}」 / {path} ({})",
                    notice.layer_name, notice.reason
                ),
            );
            if ui.button("画像を差し替える").clicked() {
                replacement = Some(notice.id.clone());
            }
        });
    }
    replacement
}

struct LayerEditResult {
    changed: bool,
    commands: Vec<EditorCommand>,
    import_lrc_requested: Option<LayerId>,
    export_lrc_requested: Option<LayerId>,
}

fn edit_layer_properties(
    ui: &mut egui::Ui,
    layer: &mut Layer,
    state: &mut EditorState,
    current_time_secs: f64,
    output_width: u32,
    output_height: u32,
    modulators: &[musicvideo_core::Modulator],
    modulator_values: &[ModulatorValue],
) -> LayerEditResult {
    let mut changed = ui.text_edit_singleline(&mut layer.name).changed();
    let mut commands = Vec::new();
    let mut import_lrc_requested = None;
    let mut export_lrc_requested = None;
    for property in keyframes::animation_properties(layer) {
        edit_animatable_property(
            ui,
            layer,
            property,
            state.selected_key(),
            current_time_secs,
            output_width,
            output_height,
            &mut commands,
        );
    }
    ui.separator();
    changed |= match &mut layer.kind {
        LayerKind::Solid { .. } | LayerKind::Image { .. } => false,
        LayerKind::Spectrum(spectrum) => edit_spectrum(ui, spectrum),
        LayerKind::PitchKeyboard(keyboard) => edit_music_color_mode(ui, &mut keyboard.color_mode),
        LayerKind::ProgressBar(progress) => ui
            .add(
                egui::DragValue::new(&mut progress.border_thickness_px)
                    .range(0.0..=100.0)
                    .suffix(" px 枠"),
            )
            .changed(),
        LayerKind::TimeText(text) => edit_time_text(ui, text),
        LayerKind::Text(text) => edit_text(ui, text),
        LayerKind::Lyrics(lyrics) => {
            let result = edit_lyrics(
                ui,
                layer.id,
                lyrics,
                state,
                current_time_secs,
                &mut commands,
            );
            import_lrc_requested = result.import_requested.then_some(layer.id);
            export_lrc_requested = result.export_requested.then_some(layer.id);
            result.changed
        }
    };
    changed |= modulators::draw_bindings(ui, layer, modulators, modulator_values);
    LayerEditResult {
        changed,
        commands,
        import_lrc_requested,
        export_lrc_requested,
    }
}

#[allow(clippy::too_many_arguments)]
fn edit_animatable_property(
    ui: &mut egui::Ui,
    layer: &Layer,
    property: AnimProperty,
    selected_key: Option<(AnimProperty, usize)>,
    current_time_secs: f64,
    output_width: u32,
    output_height: u32,
    commands: &mut Vec<EditorCommand>,
) {
    let keys = keyframes::animation_keys(layer, property).unwrap_or_default();
    let selected_index = selected_key
        .filter(|(selected_property, _)| *selected_property == property)
        .map(|(_, key_index)| key_index);
    let Some(mut value) =
        keyframes::animation_value(layer, property, current_time_secs, selected_index)
    else {
        return;
    };
    egui::CollapsingHeader::new(property.label())
        .id_salt(("animatable-property", layer.id.0, property))
        .default_open(matches!(
            property,
            AnimProperty::Visible
                | AnimProperty::Opacity
                | AnimProperty::Rect
                | AnimProperty::ImageZoom
        ))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.weak(if keys.is_empty() {
                    "定数".to_owned()
                } else {
                    format!("トラック ({} キー)", keys.len())
                });
                if ui.button("◆ 現在にキー").clicked() {
                    commands.push(EditorCommand::AddKey {
                        layer: layer.id,
                        property,
                        t: current_time_secs,
                    });
                }
                if !keys.is_empty() && ui.button("定数に戻す").clicked() {
                    commands.push(EditorCommand::CollapseToConst {
                        layer: layer.id,
                        property,
                        t: current_time_secs,
                    });
                }
                if let Some(index) = selected_index {
                    ui.label(format!("選択キー #{}", index + 1));
                }
            });
            if edit_animation_value(ui, property, &mut value, output_width, output_height) {
                commands.push(EditorCommand::SetValue {
                    layer: layer.id,
                    property,
                    t: current_time_secs,
                    key_index: selected_index,
                    value,
                });
            }
        });
}

fn edit_animation_value(
    ui: &mut egui::Ui,
    property: AnimProperty,
    value: &mut AnimationValue,
    output_width: u32,
    output_height: u32,
) -> bool {
    match (property, value) {
        (AnimProperty::Visible, AnimationValue::Bool(value)) => {
            ui.checkbox(value, "表示する").changed()
        }
        (AnimProperty::Opacity, AnimationValue::Float(value)) => ui
            .add(egui::Slider::new(value, 0.0..=1.0).text("不透明度"))
            .changed(),
        (AnimProperty::ImageZoom, AnimationValue::Float(value)) => ui
            .add(egui::Slider::new(value, 0.1..=4.0).text("絵そのものの拡大"))
            .on_hover_text("レイヤーの矩形は動かさず、その内側で画像だけを中心基準に拡大します")
            .changed(),
        (AnimProperty::Rotation, AnimationValue::Float(value)) => ui
            .add(
                egui::DragValue::new(value)
                    .range(-180.0..=180.0)
                    .suffix("°"),
            )
            .changed(),
        (AnimProperty::CornerRadius, AnimationValue::Float(value)) => {
            drag_pixels(ui, "半径", value, 0.0..=1000.0)
        }
        (AnimProperty::Rect, AnimationValue::Rect(rect)) => {
            edit_rect(ui, rect, output_width, output_height)
        }
        (AnimProperty::Glow, AnimationValue::Glow(glow)) => edit_glow(ui, glow),
        (_, AnimationValue::Fill(fill)) => edit_fill(ui, fill),
        _ => false,
    }
}

fn edit_time_text(ui: &mut egui::Ui, text: &mut TimeTextLayer) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("表示形式");
        egui::ComboBox::from_id_salt("time-text-format")
            .selected_text(match text.format {
                TimeFormat::MmSs => "経過時間",
                TimeFormat::MmSsOfTotal => "経過時間 / 全体",
            })
            .show_ui(ui, |ui| {
                changed |= ui
                    .selectable_value(&mut text.format, TimeFormat::MmSs, "経過時間")
                    .changed();
                changed |= ui
                    .selectable_value(&mut text.format, TimeFormat::MmSsOfTotal, "経過時間 / 全体")
                    .changed();
            });
    });
    changed |= edit_text_layout(ui, &mut text.size_px, &mut text.align);
    changed
}

fn edit_text(ui: &mut egui::Ui, text: &mut TextLayer) -> bool {
    let mut changed = ui.text_edit_singleline(&mut text.text).changed();
    changed |= edit_text_layout(ui, &mut text.size_px, &mut text.align);
    changed
}

#[derive(Default)]
struct LyricsEditResult {
    changed: bool,
    import_requested: bool,
    export_requested: bool,
}

fn edit_lyrics(
    ui: &mut egui::Ui,
    layer_id: LayerId,
    lyrics: &mut LyricsLayer,
    state: &mut EditorState,
    current_time_secs: f64,
    commands: &mut Vec<EditorCommand>,
) -> LyricsEditResult {
    let mut result = LyricsEditResult::default();
    ui.heading("歌詞");
    ui.small(
        "1 行ずつ貼り付け、再生しながら「次の行へ」で時刻を付けます。空行は歌詞を消す区間です。",
    );

    let mut block = lyrics
        .lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if ui
        .add(
            egui::TextEdit::multiline(&mut block)
                .desired_rows(7)
                .desired_width(f32::INFINITY)
                .hint_text("ここに歌詞をまとめて貼り付け"),
        )
        .changed()
    {
        commands.push(EditorCommand::ReplaceLyricsText {
            layer: layer_id,
            text: block,
        });
    }

    let next_unset = lyrics
        .lines
        .iter()
        .position(|line| line.time_secs.is_none());
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(next_unset.is_some(), egui::Button::new("次の行へ"))
            .on_hover_text("次の未設定行へ、音声の現在時刻を記録します")
            .clicked()
            && let Some(line_index) = next_unset
        {
            commands.push(EditorCommand::SetLyricTime {
                layer: layer_id,
                line_index,
                time_secs: Some(current_time_secs),
            });
            state.selected_detail = Some(DetailSelection::Lyric { line_index });
            state.dragging_detail = None;
        }
    });
    ui.small("時刻付けを戻すときは、上の「元に戻す」または Ctrl+Z を使います。");
    if let Some(index) = next_unset {
        let preview = lyrics.lines[index].text.replace('\n', " ");
        ui.small(format!("次: {} / {}", index + 1, preview));
    } else {
        ui.small("すべての行に時刻が設定されています");
    }

    ui.horizontal(|ui| {
        if ui.button("LRC を読み込む…").clicked() {
            result.import_requested = true;
        }
        if ui.button("LRC に書き出す…").clicked() {
            result.export_requested = true;
        }
    });
    if let Some((_, notice)) = state
        .lyrics_notice
        .as_ref()
        .filter(|(notice_layer, _)| *notice_layer == layer_id)
    {
        ui.small(notice);
    }

    ui.collapsing("行ごとの時刻を修正", |ui| {
        for (line_index, line) in lyrics.lines.iter().enumerate() {
            ui.horizontal(|ui| {
                let selected = state.selected_lyric() == Some(line_index);
                if ui
                    .selectable_label(selected, format!("{}", line_index + 1))
                    .clicked()
                {
                    state.selected_detail = Some(DetailSelection::Lyric { line_index });
                    state.dragging_detail = None;
                }
                if let Some(original_time) = line.time_secs {
                    let mut time = original_time;
                    if ui
                        .add(
                            egui::DragValue::new(&mut time)
                                .range(0.0..=f64::MAX)
                                .speed(0.01)
                                .suffix(" 秒"),
                        )
                        .changed()
                    {
                        commands.push(EditorCommand::SetLyricTime {
                            layer: layer_id,
                            line_index,
                            time_secs: Some(time),
                        });
                    }
                    if ui.small_button("未設定に戻す").clicked() {
                        commands.push(EditorCommand::SetLyricTime {
                            layer: layer_id,
                            line_index,
                            time_secs: None,
                        });
                    }
                } else {
                    ui.weak("未設定");
                    if ui.small_button("現在時刻を設定").clicked() {
                        commands.push(EditorCommand::SetLyricTime {
                            layer: layer_id,
                            line_index,
                            time_secs: Some(current_time_secs),
                        });
                    }
                }
                let text = if line.text.is_empty() {
                    "（空行）"
                } else {
                    line.text.as_str()
                };
                ui.label(text);
            });
        }
    });

    ui.separator();
    result.changed |= ui
        .add(
            egui::DragValue::new(&mut lyrics.fade_in_secs)
                .range(0.0..=10.0)
                .speed(0.01)
                .suffix(" 秒 フェードイン"),
        )
        .changed();
    result.changed |= ui
        .add(
            egui::DragValue::new(&mut lyrics.fade_out_secs)
                .range(0.0..=10.0)
                .speed(0.01)
                .suffix(" 秒 フェードアウト"),
        )
        .changed();
    result.changed |= edit_text_layout(ui, &mut lyrics.size_px, &mut lyrics.align);
    result
}

fn edit_text_layout(ui: &mut egui::Ui, size_px: &mut f32, align: &mut TextAlign) -> bool {
    let mut changed = ui
        .add(
            egui::DragValue::new(size_px)
                .range(1.0..=1000.0)
                .suffix(" px 文字サイズ"),
        )
        .changed();
    ui.horizontal(|ui| {
        ui.label("文字揃え");
        changed |= ui.radio_value(align, TextAlign::Start, "左").changed();
        changed |= ui.radio_value(align, TextAlign::Center, "中央").changed();
        changed |= ui.radio_value(align, TextAlign::End, "右").changed();
    });
    changed
}

fn edit_rect(
    ui: &mut egui::Ui,
    rect: &mut musicvideo_core::NormRect,
    width: u32,
    height: u32,
) -> bool {
    let width = width.max(1) as f32;
    let height = height.max(1) as f32;
    let mut pixels = [
        rect.x * width,
        rect.y * height,
        rect.width * width,
        rect.height * height,
    ];
    let mut changed = false;
    for (label, value) in ["X", "Y", "幅", "高さ"].into_iter().zip(&mut pixels) {
        changed |= ui
            .add(
                egui::DragValue::new(value)
                    .speed(1.0)
                    .prefix(format!("{label} "))
                    .suffix(" px"),
            )
            .changed();
    }
    if changed {
        *rect = musicvideo_core::NormRect::new(
            pixels[0] / width,
            pixels[1] / height,
            (pixels[2] / width).max(0.0),
            (pixels[3] / height).max(0.0),
        );
    }
    changed
}

fn edit_glow(ui: &mut egui::Ui, glow: &mut Option<Glow>) -> bool {
    let mut enabled = glow.is_some();
    let mut changed = ui.checkbox(&mut enabled, "グロー").changed();
    if enabled && glow.is_none() {
        *glow = Some(Glow {
            color: Rgba8::WHITE,
            radius_px: 12.0,
            spread_px: 0.0,
        });
        changed = true;
    } else if !enabled && glow.is_some() {
        *glow = None;
        changed = true;
    }
    if let Some(glow) = glow {
        changed |= edit_color(ui, "グロー色", &mut glow.color);
        changed |= ui
            .add(
                egui::DragValue::new(&mut glow.radius_px)
                    .range(0.0..=128.0)
                    .suffix(" px 半径"),
            )
            .changed();
        changed |= ui
            .add(
                egui::DragValue::new(&mut glow.spread_px)
                    .range(0.0..=128.0)
                    .suffix(" px 広がり"),
            )
            .changed();
    }
    changed
}

fn edit_color(ui: &mut egui::Ui, label: &str, color: &mut Rgba8) -> bool {
    let mut value = egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a);
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        changed = ui.color_edit_button_srgba(&mut value).changed();
    });
    if changed {
        *color = Rgba8::new(value.r(), value.g(), value.b(), value.a());
    }
    changed
}

fn edit_spectrum(ui: &mut egui::Ui, spectrum: &mut SpectrumLayer) -> bool {
    let mut changed = ui
        .add(
            egui::DragValue::new(&mut spectrum.band_count)
                .range(1..=128)
                .prefix("解析バンド "),
        )
        .changed();
    if spectrum.shape != SpectrumShape::MivHorizontal {
        changed |= ui
            .add(
                egui::DragValue::new(&mut spectrum.point_count)
                    .range(1..=4096)
                    .prefix("描画点 "),
            )
            .changed();
    }
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum DisplayMode {
        EqualBars,
        MivHorizontal,
        Radial,
    }
    let mut mode = match spectrum.shape {
        SpectrumShape::Bars { .. } => DisplayMode::EqualBars,
        SpectrumShape::MivHorizontal => DisplayMode::MivHorizontal,
        SpectrumShape::Radial { .. } => DisplayMode::Radial,
    };
    let previous_mode = mode;
    ui.horizontal(|ui| {
        ui.radio_value(&mut mode, DisplayMode::EqualBars, "等間隔の直線");
        ui.radio_value(&mut mode, DisplayMode::MivHorizontal, "mIV と同じ横長");
        ui.radio_value(&mut mode, DisplayMode::Radial, "円形");
    });
    if mode != previous_mode {
        spectrum.shape = match mode {
            DisplayMode::Radial => SpectrumShape::Radial {
                radius_px: 380.0,
                thickness_px: 4.0,
                start_angle_deg: -90.0,
                sweep_deg: 360.0,
                direction: SpectrumDirection::Clockwise,
            },
            DisplayMode::EqualBars => SpectrumShape::Bars {
                gap_ratio: 0.3,
                min_height_px: 3.0,
                thickness_px: 12.0,
            },
            DisplayMode::MivHorizontal => {
                spectrum.band_count = MIV_SPECTRUM_BANDS;
                spectrum.point_count = MIV_SPECTRUM_BANDS;
                SpectrumShape::MivHorizontal
            }
        };
        changed = true;
    }
    match &mut spectrum.shape {
        SpectrumShape::Bars {
            gap_ratio,
            min_height_px,
            thickness_px,
        } => {
            changed |= ui
                .add(egui::Slider::new(gap_ratio, 0.0..=1.0).text("隙間"))
                .changed();
            changed |= drag_pixels(ui, "最小高さ", min_height_px, 0.0..=1000.0);
            changed |= drag_pixels(ui, "太さ", thickness_px, 0.5..=200.0);
        }
        SpectrumShape::Radial {
            radius_px,
            thickness_px,
            start_angle_deg,
            sweep_deg,
            direction,
        } => {
            changed |= drag_pixels(ui, "半径", radius_px, 0.0..=2000.0);
            changed |= drag_pixels(ui, "太さ", thickness_px, 0.5..=200.0);
            changed |= ui
                .add(egui::DragValue::new(start_angle_deg).suffix("° 開始"))
                .changed();
            changed |= ui
                .add(
                    egui::DragValue::new(sweep_deg)
                        .range(0.0..=360.0)
                        .suffix("° 範囲"),
                )
                .changed();
            ui.horizontal(|ui| {
                changed |= ui
                    .radio_value(direction, SpectrumDirection::Clockwise, "時計回り")
                    .changed();
                changed |= ui
                    .radio_value(direction, SpectrumDirection::CounterClockwise, "反時計回り")
                    .changed();
            });
        }
        SpectrumShape::MivHorizontal => {
            ui.weak("対数周波数軸・3 枚の残光・立ち上がり強調を mIV 本体と共有します");
        }
    }
    changed |= edit_music_color_mode(ui, &mut spectrum.color_mode);
    changed
}

fn edit_music_color_mode(ui: &mut egui::Ui, mode: &mut MusicColorMode) -> bool {
    let previous = *mode;
    ui.horizontal(|ui| {
        ui.label("配色");
        egui::ComboBox::from_id_salt("music-color-mode")
            .selected_text(match mode {
                MusicColorMode::Custom => "カスタム",
                MusicColorMode::Miv => "mIV 本体と同じ",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(mode, MusicColorMode::Custom, "カスタム");
                ui.selectable_value(mode, MusicColorMode::Miv, "mIV 本体と同じ");
            });
    });
    *mode != previous
}

fn drag_pixels(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
) -> bool {
    ui.add(
        egui::DragValue::new(value)
            .range(range)
            .prefix(format!("{label} "))
            .suffix(" px"),
    )
    .changed()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FillKind {
    Solid,
    Linear,
    Radial,
    Band,
}

fn edit_fill(ui: &mut egui::Ui, fill: &mut Fill) -> bool {
    let mut kind = match fill {
        Fill::Solid(_) => FillKind::Solid,
        Fill::LinearGradient { .. } => FillKind::Linear,
        Fill::RadialGradient { .. } => FillKind::Radial,
        Fill::BandGradient { .. } => FillKind::Band,
    };
    let previous = kind;
    egui::ComboBox::from_id_salt("fill-kind")
        .selected_text(fill_kind_label(kind))
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut kind, FillKind::Solid, "単色");
            ui.selectable_value(&mut kind, FillKind::Linear, "線形グラデーション");
            ui.selectable_value(&mut kind, FillKind::Radial, "放射グラデーション");
            ui.selectable_value(&mut kind, FillKind::Band, "バンドグラデーション");
        });
    let mut changed = kind != previous;
    if changed {
        *fill = default_fill(kind);
    }
    match fill {
        Fill::Solid(color) => changed |= edit_color(ui, "色", color),
        Fill::LinearGradient { stops, angle_deg } => {
            changed |= ui
                .add(egui::DragValue::new(angle_deg).suffix("° 角度"))
                .changed();
            changed |= edit_gradient_stops(ui, stops);
        }
        Fill::RadialGradient {
            stops,
            center,
            radius,
        } => {
            changed |= ui
                .add(egui::Slider::new(&mut center.0, 0.0..=1.0).text("中心 X "))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut center.1, 0.0..=1.0).text("中心 Y "))
                .changed();
            changed |= ui
                .add(egui::Slider::new(radius, 0.01..=2.0).text("半径"))
                .changed();
            changed |= edit_gradient_stops(ui, stops);
        }
        Fill::BandGradient { stops } => changed |= edit_gradient_stops(ui, stops),
    }
    changed
}

fn fill_kind_label(kind: FillKind) -> &'static str {
    match kind {
        FillKind::Solid => "単色",
        FillKind::Linear => "線形",
        FillKind::Radial => "放射",
        FillKind::Band => "バンド",
    }
}

fn default_fill(kind: FillKind) -> Fill {
    match kind {
        FillKind::Solid => Fill::Solid(Rgba8::WHITE),
        FillKind::Linear => Fill::LinearGradient {
            stops: default_gradient_stops(),
            angle_deg: 0.0,
        },
        FillKind::Radial => Fill::RadialGradient {
            stops: default_gradient_stops(),
            center: (0.5, 0.5),
            radius: 0.5,
        },
        FillKind::Band => Fill::BandGradient {
            stops: default_gradient_stops(),
        },
    }
}

fn default_gradient_stops() -> GradientStops {
    GradientStops::new(vec![
        GradientStop {
            position: 0.0,
            color: Rgba8::new(120, 255, 35, 255),
        },
        GradientStop {
            position: 1.0,
            color: Rgba8::new(255, 136, 255, 255),
        },
    ])
    .expect("fixed gradient stops are valid")
}

fn edit_gradient_stops(ui: &mut egui::Ui, stops: &mut GradientStops) -> bool {
    let mut values = stops.to_vec();
    let mut changed = false;
    let mut remove = None;
    let can_remove = values.len() > 1;
    for (index, stop) in values.iter_mut().enumerate() {
        ui.push_id(index, |ui| {
            ui.horizontal(|ui| {
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut stop.position)
                            .range(0.0..=1.0)
                            .speed(0.01),
                    )
                    .changed();
                changed |= edit_color(ui, "色", &mut stop.color);
                if can_remove && ui.small_button("削除").clicked() {
                    remove = Some(index);
                }
            });
        });
    }
    if let Some(index) = remove {
        values.remove(index);
        changed = true;
    }
    if ui.small_button("ストップを追加").clicked() {
        values.push(GradientStop {
            position: 1.0,
            color: Rgba8::WHITE,
        });
        changed = true;
    }
    if changed {
        *stops = GradientStops::new(values).expect("UI restricts gradient stop positions");
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use musicvideo_core::{
        Animatable, AudioRef, ImageRef, Interp, Keyframe, LyricLine, ModulationBinding,
        ModulationCurve, ModulationTarget, Track, default_project, new_band_energy_modulator,
        new_image_layer, new_solid_layer,
    };
    use std::sync::Arc;

    fn assert_undo_redo_round_trip(
        state: &mut EditorState,
        project: &mut Project,
        before: &Project,
        after: &Project,
    ) {
        assert!(state.undo(project));
        assert_eq!(project, before);
        assert!(state.redo(project));
        assert_eq!(project, after);
    }

    fn record_direct_edit(
        state: &mut EditorState,
        project: &mut Project,
        edit: impl FnOnce(&mut Project),
    ) {
        let checkpoint = state.begin_external_edit(project);
        edit(project);
        assert!(state.finish_external_edit(project, checkpoint));
    }

    #[test]
    fn new_keyboard_layer_uses_a_transparent_background_and_unique_id() {
        let project = default_project(10.0);
        let layer = new_pitch_keyboard_layer(&project);
        assert_eq!(layer.id.0, 5);
        let LayerKind::PitchKeyboard(keyboard) = layer.kind else {
            panic!("追加レイヤはピッチ鍵盤である必要があります");
        };
        assert_eq!(
            keyboard.background,
            Animatable::Const(Fill::Solid(Rgba8::new(0, 0, 0, 0)))
        );
    }

    #[test]
    fn add_layer_menu_lists_every_model_layer_type() {
        assert_eq!(
            NEW_LAYER_PRESETS.map(|(_, label)| label),
            [
                "画像",
                "円形スペクトラム",
                "直線スペクトラム",
                "mIV と同じ横長スペクトラム",
                "mIV スペクトラム + 鍵盤",
                "ピッチ鍵盤",
                "進捗バー",
                "時間表示",
                "テキスト",
                "歌詞",
                "単色",
            ]
        );
    }

    #[test]
    fn layer_order_command_keeps_the_same_layer_selected() {
        let mut project = default_project(10.0);
        let mut state = EditorState::default();
        state.select_layer(&project, 1);
        let selected = project.layers[1].id;
        assert!(state.apply_external_command(
            &mut project,
            &EditorCommand::AddKey {
                layer: selected,
                property: AnimProperty::Opacity,
                t: 1.0,
            },
        ));

        assert!(state.apply_command(
            &mut project,
            &EditorCommand::MoveLayer {
                layer: selected,
                direction: LayerMoveDirection::TowardFront,
            },
        ));

        assert_eq!(state.selected_layer(&project), 2);
        assert_eq!(project.layers[state.selected_layer(&project)].id, selected);
        assert_eq!(state.selected_key(), Some((AnimProperty::Opacity, 0)));
    }

    #[test]
    fn selecting_another_layer_clears_the_previous_layers_detail() {
        let mut project = default_project(10.0);
        let mut lyrics = new_lyrics_layer(&project);
        let LayerKind::Lyrics(lyrics_body) = &mut lyrics.kind else {
            unreachable!();
        };
        lyrics_body.lines.push(LyricLine {
            text: "line".to_owned(),
            time_secs: Some(1.0),
        });
        project.layers.push(lyrics);
        let lyrics_index = project.layers.len() - 1;
        let lyrics_id = project.layers[lyrics_index].id;
        let mut state = EditorState::default();

        state.select_lyric(&project, lyrics_id, 0);
        assert_eq!(state.selected_layer(&project), lyrics_index);
        assert_eq!(state.selected_lyric(), Some(0));

        state.select_layer(&project, 0);
        assert_eq!(state.selected_detail, None);
        assert_eq!(state.dragging_detail, None);
    }

    #[test]
    fn deleting_a_selected_layer_does_not_attach_its_detail_to_a_sibling() {
        let mut project = default_project(10.0);
        let mut first = new_lyrics_layer(&project);
        let first_id = first.id;
        let LayerKind::Lyrics(first_lyrics) = &mut first.kind else {
            unreachable!();
        };
        first_lyrics.lines.push(LyricLine {
            text: "same".to_owned(),
            time_secs: Some(1.0),
        });
        project.layers.push(first);
        let mut second = new_lyrics_layer(&project);
        let LayerKind::Lyrics(second_lyrics) = &mut second.kind else {
            unreachable!();
        };
        second_lyrics.lines.push(LyricLine {
            text: "same".to_owned(),
            time_secs: Some(2.0),
        });
        project.layers.push(second);
        let mut state = EditorState::default();
        state.select_lyric(&project, first_id, 0);

        assert!(state.apply_external_command(
            &mut project,
            &EditorCommand::DeleteLayer { layer: first_id },
        ));

        assert_eq!(state.selected_detail, None);
        assert_eq!(state.dragging_detail, None);
    }

    #[test]
    fn detail_selection_is_restored_with_its_parent_layer_by_undo() {
        let mut project = default_project(10.0);
        let mut state = EditorState::default();
        let layer = project.layers[0].id;
        state.select_layer(&project, 0);
        assert!(state.apply_external_command(
            &mut project,
            &EditorCommand::AddKey {
                layer,
                property: AnimProperty::Opacity,
                t: 1.0,
            },
        ));
        assert_eq!(state.selected_key(), Some((AnimProperty::Opacity, 0)));

        let checkpoint = state.begin_external_edit(&project);
        state.select_layer(&project, 1);
        project.layers[1].name.push_str(" edited");
        assert!(state.finish_external_edit(&project, checkpoint));
        assert!(state.undo(&mut project));

        assert_eq!(project.layers[state.selected_layer(&project)].id, layer);
        assert_eq!(state.selected_key(), Some((AnimProperty::Opacity, 0)));
        assert_eq!(state.selected_lyric(), None);
    }

    #[test]
    fn project_replacement_clears_a_pending_delete_even_when_layer_ids_repeat() {
        let first = default_project(10.0);
        let second = default_project(20.0);
        let mut state = EditorState::default();
        state.pending_layer_delete = Some(first.layers[1].id);

        state.reset_for_project(&second);

        assert_eq!(state.pending_layer_delete, None);
    }

    #[test]
    fn key_add_move_and_delete_each_round_trip_through_undo_and_redo() {
        let mut project = default_project(10.0);
        let mut state = EditorState::default();
        state.select_layer(&project, 0);
        let layer = project.layers[0].id;

        let before_add = project.clone();
        assert!(state.apply_external_command(
            &mut project,
            &EditorCommand::AddKey {
                layer,
                property: AnimProperty::Opacity,
                t: 1.0,
            },
        ));
        let after_add = project.clone();
        assert_undo_redo_round_trip(&mut state, &mut project, &before_add, &after_add);
        assert_eq!(state.selected_key(), Some((AnimProperty::Opacity, 0)));

        let before_move = project.clone();
        assert!(state.apply_external_command(
            &mut project,
            &EditorCommand::MoveKey {
                layer,
                property: AnimProperty::Opacity,
                key_index: 0,
                t: 3.0,
            },
        ));
        let after_move = project.clone();
        assert_undo_redo_round_trip(&mut state, &mut project, &before_move, &after_move);

        let before_delete = project.clone();
        assert!(state.apply_external_command(
            &mut project,
            &EditorCommand::DeleteKey {
                layer,
                property: AnimProperty::Opacity,
                key_index: 0,
            },
        ));
        let after_delete = project.clone();
        assert_eq!(project.layers[0].opacity, Animatable::Const(1.0));
        assert_undo_redo_round_trip(&mut state, &mut project, &before_delete, &after_delete);
    }

    #[test]
    fn layer_add_reorder_and_delete_each_round_trip() {
        let mut project = default_project(10.0);
        let mut state = EditorState::default();

        let before_add = project.clone();
        let added = new_solid_layer(&project);
        let added_id = added.id;
        let checkpoint = state.begin_external_edit(&project);
        project.layers.push(added);
        state.select_layer(&project, project.layers.len() - 1);
        assert!(state.finish_external_edit(&project, checkpoint));
        let after_add = project.clone();
        assert_undo_redo_round_trip(&mut state, &mut project, &before_add, &after_add);
        assert_eq!(project.layers[state.selected_layer(&project)].id, added_id);

        let before_move = project.clone();
        assert!(state.apply_external_command(
            &mut project,
            &EditorCommand::MoveLayer {
                layer: added_id,
                direction: LayerMoveDirection::TowardBack,
            },
        ));
        let after_move = project.clone();
        assert_undo_redo_round_trip(&mut state, &mut project, &before_move, &after_move);

        let before_delete = project.clone();
        assert!(state.apply_external_command(
            &mut project,
            &EditorCommand::DeleteLayer { layer: added_id },
        ));
        let after_delete = project.clone();
        assert_undo_redo_round_trip(&mut state, &mut project, &before_delete, &after_delete);
    }

    #[test]
    fn deleting_an_image_layer_undoes_bindings_and_last_image_reference_together() {
        let mut project = default_project(10.0);
        let modulator = new_band_energy_modulator(&project);
        let modulator_id = modulator.id;
        project.modulators.push(modulator);
        let image = ImageId::new("undo-image");
        project.images.push(ImageRef {
            id: image.clone(),
            path: PathBuf::from("cover.png"),
        });
        let mut layer = new_image_layer(&project, image.clone());
        layer.modulations.push(ModulationBinding {
            target: ModulationTarget::ImageZoom,
            modulator: modulator_id,
            amount: 0.2,
            curve: ModulationCurve::Squared,
        });
        let layer_id = layer.id;
        project.layers.push(layer);
        let mut state = EditorState::default();
        state.select_layer(&project, project.layers.len() - 1);
        state.reset_for_project(&project);
        state.select_layer(&project, project.layers.len() - 1);
        let before = project.clone();

        assert!(state.apply_external_command(
            &mut project,
            &EditorCommand::DeleteLayer { layer: layer_id },
        ));
        let deleted = project.clone();
        assert!(!project.images.iter().any(|reference| reference.id == image));
        assert!(!project.layers.iter().any(|layer| layer.id == layer_id));

        assert!(state.undo(&mut project));
        assert_eq!(project, before);
        let restored = project
            .layers
            .iter()
            .find(|layer| layer.id == layer_id)
            .unwrap();
        assert_eq!(restored.modulations.len(), 1);
        assert!(project.images.iter().any(|reference| reference.id == image));
        assert_eq!(project.layers[state.selected_layer(&project)].id, layer_id);
        assert!(state.redo(&mut project));
        assert_eq!(project, deleted);
    }

    #[test]
    fn property_and_modulator_edits_round_trip() {
        let mut project = default_project(10.0);
        let mut state = EditorState::default();

        let before_property = project.clone();
        record_direct_edit(&mut state, &mut project, |project| {
            project.layers[0].name = "変更したレイヤー".to_owned();
        });
        let after_property = project.clone();
        assert_undo_redo_round_trip(&mut state, &mut project, &before_property, &after_property);

        let before_modulator = project.clone();
        record_direct_edit(&mut state, &mut project, |project| {
            project.modulators.push(new_band_energy_modulator(project));
        });
        let after_modulator = project.clone();
        assert_undo_redo_round_trip(
            &mut state,
            &mut project,
            &before_modulator,
            &after_modulator,
        );
    }

    #[test]
    fn lyric_bulk_replacement_and_timestamp_round_trip() {
        let mut project = default_project(10.0);
        let lyrics = new_lyrics_layer(&project);
        let layer = lyrics.id;
        project.layers.push(lyrics);
        let mut state = EditorState::default();
        state.reset_for_project(&project);

        let before_replace = project.clone();
        assert!(state.apply_external_command(
            &mut project,
            &EditorCommand::ReplaceLyrics {
                layer,
                lines: vec![
                    LyricLine {
                        text: "一行目".to_owned(),
                        time_secs: None,
                    },
                    LyricLine {
                        text: String::new(),
                        time_secs: None,
                    },
                ],
            },
        ));
        let after_replace = project.clone();
        assert_undo_redo_round_trip(&mut state, &mut project, &before_replace, &after_replace);

        let before_stamp = project.clone();
        assert!(state.apply_external_command(
            &mut project,
            &EditorCommand::SetLyricTime {
                layer,
                line_index: 0,
                time_secs: Some(2.5),
            },
        ));
        let after_stamp = project.clone();
        assert_undo_redo_round_trip(&mut state, &mut project, &before_stamp, &after_stamp);
    }

    #[test]
    fn one_timeline_marker_drag_creates_exactly_one_history_step() {
        let mut project = default_project(10.0);
        project.layers[0].opacity = Animatable::Track(
            Track::new(vec![Keyframe {
                t: 1.0,
                value: 0.5,
                interp: Interp::Linear,
            }])
            .unwrap(),
        );
        let layer = project.layers[0].id;
        let original = project.clone();
        let mut state = EditorState::default();
        state.reset_for_project(&project);

        for t in [1.5, 2.0, 3.0] {
            let checkpoint = state.begin_external_edit(&project);
            assert!(state.apply_command(
                &mut project,
                &EditorCommand::MoveKey {
                    layer,
                    property: AnimProperty::Opacity,
                    key_index: 0,
                    t,
                },
            ));
            state.observe_ui_pass(
                EditSurface::Timeline,
                &project,
                checkpoint,
                true,
                true,
                None,
            );
        }
        let release = state.begin_external_edit(&project);
        state.observe_ui_pass(EditSurface::Timeline, &project, release, false, false, None);

        assert_eq!(state.history_depths(), (1, 0));
        let final_project = project.clone();
        assert_undo_redo_round_trip(&mut state, &mut project, &original, &final_project);
    }

    #[test]
    fn one_text_focus_session_creates_exactly_one_history_step() {
        let mut project = default_project(10.0);
        let original = project.clone();
        let mut state = EditorState::default();

        for name in ["編", "編集中", "編集中の名前"] {
            let checkpoint = state.begin_external_edit(&project);
            project.layers[0].name = name.to_owned();
            state.observe_ui_pass(
                EditSurface::Properties,
                &project,
                checkpoint,
                true,
                false,
                Some(egui::Id::new("layer-name")),
            );
        }
        let focus_lost = state.begin_external_edit(&project);
        state.observe_ui_pass(
            EditSurface::Properties,
            &project,
            focus_lost,
            false,
            false,
            None,
        );

        assert_eq!(state.history_depths(), (1, 0));
        assert!(state.undo(&mut project));
        assert_eq!(project, original);
    }

    #[test]
    fn moving_from_a_text_field_to_the_timeline_keeps_two_operations() {
        let mut project = default_project(10.0);
        let original = project.clone();
        let mut state = EditorState::default();

        let text_start = state.begin_external_edit(&project);
        project.layers[0].name = "入力後".to_owned();
        state.observe_ui_pass(
            EditSurface::Properties,
            &project,
            text_start,
            true,
            false,
            Some(egui::Id::new("layer-name")),
        );
        let after_text = project.clone();

        let timeline_start = state.begin_external_edit(&project);
        project.display_gain_db = 4.0;
        state.observe_ui_pass(
            EditSurface::Timeline,
            &project,
            timeline_start,
            true,
            true,
            None,
        );
        let release = state.begin_external_edit(&project);
        state.observe_ui_pass(EditSurface::Timeline, &project, release, false, false, None);

        assert_eq!(state.history_depths(), (2, 0));
        assert!(state.undo(&mut project));
        assert_eq!(project, after_text);
        assert!(state.undo(&mut project));
        assert_eq!(project, original);
    }

    #[test]
    fn loading_a_project_clears_both_history_stacks() {
        let mut project = default_project(10.0);
        let mut state = EditorState::default();
        record_direct_edit(&mut state, &mut project, |project| {
            project.display_gain_db = 6.0;
        });
        assert!(state.undo(&mut project));
        assert_eq!(state.history_depths(), (0, 1));

        let loaded = default_project(20.0);
        state.reset_for_project(&loaded);
        project = loaded;
        assert_eq!(state.history_depths(), (0, 0));
        assert!(!state.undo(&mut project));
        assert!(!state.redo(&mut project));
    }

    #[test]
    fn resource_manifest_and_project_use_the_same_undo_entry() {
        let mut project = default_project(10.0);
        project.audio = Some(AudioRef {
            path: PathBuf::from("first.wav"),
        });
        let first = ResourceLease::new(Arc::new(()));
        let second = ResourceLease::new(Arc::new(()));
        let mut first_manifest = ResourceManifest::default();
        first_manifest.set_audio(Some(first));
        let mut state = EditorState::default();
        state.reset_for_project_with_resources(&project, first_manifest.clone());

        let checkpoint = state.begin_external_edit(&project);
        project.audio = Some(AudioRef {
            path: PathBuf::from("second.wav"),
        });
        let mut second_manifest = first_manifest.clone();
        second_manifest.set_audio(Some(second));
        assert!(state.finish_external_edit_with_resources(
            &project,
            checkpoint,
            second_manifest.clone(),
        ));

        assert!(state.undo(&mut project));
        assert_eq!(
            project.audio.as_ref().unwrap().path,
            PathBuf::from("first.wav")
        );
        assert_eq!(state.resource_manifest(), &first_manifest);
        assert!(state.redo(&mut project));
        assert_eq!(
            project.audio.as_ref().unwrap().path,
            PathBuf::from("second.wav")
        );
        assert_eq!(state.resource_manifest(), &second_manifest);
    }

    #[test]
    fn deleting_and_recreating_ids_allocates_new_async_target_lifetimes() {
        let mut project = default_project(10.0);
        let image_id = ImageId::new("replace-me");
        project.images.push(ImageRef {
            id: image_id.clone(),
            path: PathBuf::from("first.png"),
        });
        let layer = new_image_layer(&project, image_id.clone());
        let layer_id = layer.id;
        project.layers.push(layer.clone());
        let mut state = EditorState::default();
        state.reset_for_project(&project);
        let original_layer_lifetime = state.layer_target_lifetime(layer_id).unwrap();
        let original_image_lifetime = state.image_target_lifetime(&image_id).unwrap();

        assert!(state.apply_external_command(
            &mut project,
            &EditorCommand::DeleteLayer { layer: layer_id },
        ));
        let checkpoint = state.begin_external_edit(&project);
        project.images.push(ImageRef {
            id: image_id.clone(),
            path: PathBuf::from("second.png"),
        });
        project.layers.push(layer);
        assert!(state.finish_external_edit(&project, checkpoint));

        assert_ne!(
            state.layer_target_lifetime(layer_id),
            Some(original_layer_lifetime)
        );
        assert_ne!(
            state.image_target_lifetime(&image_id),
            Some(original_image_lifetime)
        );
    }

    #[test]
    fn evicting_the_delete_snapshot_releases_its_last_resource_lease() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct DropNotice(Arc<AtomicUsize>);
        impl Drop for DropNotice {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        let drops = Arc::new(AtomicUsize::new(0));
        let image_id = ImageId::new("history-image");
        let mut project = default_project(10.0);
        project.images.push(ImageRef {
            id: image_id.clone(),
            path: PathBuf::from("history.png"),
        });
        let layer = new_image_layer(&project, image_id.clone());
        let layer_id = layer.id;
        project.layers.push(layer);
        let mut resources = ResourceManifest::default();
        resources.set_image(
            image_id,
            ResourceLease::new(Arc::new(DropNotice(Arc::clone(&drops)))),
        );
        let mut state = EditorState::default();
        state.reset_for_project_with_resources(&project, resources);

        assert!(state.apply_external_command(
            &mut project,
            &EditorCommand::DeleteLayer { layer: layer_id },
        ));
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        for step in 0..history::HISTORY_LIMIT {
            let checkpoint = state.begin_external_edit(&project);
            project.display_gain_db = step as f32 + 1.0;
            assert!(state.finish_external_edit(&project, checkpoint));
        }

        assert_eq!(state.history_depths().0, history::HISTORY_LIMIT);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn new_edit_discards_redo_and_history_evicts_the_oldest_step() {
        let mut project = default_project(10.0);
        let mut state = EditorState::default();
        for value in 1..=140 {
            record_direct_edit(&mut state, &mut project, |project| {
                project.display_gain_db = value as f32;
            });
        }
        assert_eq!(state.history_depths(), (history::HISTORY_LIMIT, 0));
        for _ in 0..history::HISTORY_LIMIT {
            assert!(state.undo(&mut project));
        }
        assert_eq!(project.display_gain_db, 12.0);
        assert!(!state.undo(&mut project));
        assert_eq!(state.history_depths(), (0, history::HISTORY_LIMIT));

        record_direct_edit(&mut state, &mut project, |project| {
            project.display_gain_db = -3.0;
        });
        assert_eq!(state.history_depths(), (1, 0));
        assert!(!state.redo(&mut project));
    }

    #[test]
    fn editor_shortcuts_are_not_taken_while_text_or_ime_input_owns_focus() {
        let ctrl_z = egui::Event::Key {
            key: egui::Key::Z,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::CTRL,
        };
        let ctrl_y = egui::Event::Key {
            key: egui::Key::Y,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::CTRL,
        };
        let ctrl_shift_z = egui::Event::Key {
            key: egui::Key::Z,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
        };
        assert_eq!(
            history_action_for_event(&ctrl_z, false),
            Some(HistoryAction::Undo)
        );
        assert_eq!(
            history_action_for_event(&ctrl_y, false),
            Some(HistoryAction::Redo)
        );
        assert_eq!(
            history_action_for_event(&ctrl_shift_z, false),
            Some(HistoryAction::Redo)
        );
        assert_eq!(history_action_for_event(&ctrl_z, true), None);
        assert_eq!(history_action_for_event(&ctrl_y, true), None);
        assert_eq!(history_action_for_event(&ctrl_shift_z, true), None);
    }

    #[test]
    fn focused_egui_text_edit_keeps_ctrl_z_for_its_own_undo() {
        let mut project = default_project(10.0);
        let mut state = EditorState::default();
        record_direct_edit(&mut state, &mut project, |project| {
            project.display_gain_db = 6.0;
        });
        let edited = project.clone();
        let context = egui::Context::default();
        let text_id = egui::Id::new("undo-owned-by-text-edit");
        let mut text = "入力中".to_owned();
        let _ = context.run(Default::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                ui.add(egui::TextEdit::singleline(&mut text).id(text_id))
                    .request_focus();
            });
        });
        assert!(context.wants_keyboard_input());

        let modifiers = egui::Modifiers::CTRL;
        let output = context.run(
            egui::RawInput {
                modifiers,
                events: vec![egui::Event::Key {
                    key: egui::Key::Z,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                ..Default::default()
            },
            |context| {
                assert!(!apply_history_shortcut(context, &mut project, &mut state));
                assert!(context.input(|input| !input.events.is_empty()));
                egui::CentralPanel::default().show(context, |ui| {
                    ui.add(egui::TextEdit::singleline(&mut text).id(text_id));
                });
            },
        );
        drop(output);
        assert_eq!(project, edited);
        assert_eq!(state.history_depths(), (1, 0));
    }
}
