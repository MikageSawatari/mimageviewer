//! ラボと本体で共有する音楽ビデオ編集 UI。
//!
//! ウィンドウ、ファイル I/O、音声再生、書き出しは呼び出し側が所有する。

/// Install the shared IME repair and globally disabled Tab traversal before the first pass.
///
/// Registration is idempotent. Shortcut ownership and native text Undo remain with the host/widgets.
pub fn install_input_policy(context: &egui::Context) {
    egui_ime::install_ime_input_policy(context);
    egui_ime::install_tab_shortcut_focus_policy(context);
}

#[cfg(test)]
fn test_context() -> egui::Context {
    let context = egui::Context::default();
    install_input_policy(&context);
    context
}

use std::collections::HashMap;
use std::path::{Path, PathBuf};

mod document;
mod history;
mod keyframes;
mod lyrics_editing;
mod modulators;
mod preview;
mod resources;
mod text_style;
mod timeline;

#[cfg(test)]
mod ime_tests;
#[cfg(test)]
mod input_source_tests;
#[cfg(test)]
mod layout_tests;

pub use document::{DocumentSession, DocumentSessionId, EditGeneration, SaveCheckpoint};
pub use keyframes::{
    AnimProperty, AnimationValue, CommandResult, EditorCommand, EditorCommandError,
    LayerMoveDirection, apply_editor_command,
};
pub use preview::{PreviewFrame, draw_preview};
pub use resources::{EditTargetLifetime, ResourceLease, ResourceManifest, ResourceRevision};
pub use timeline::{TimelineResponse, TimelineWaveform, draw_timeline};

use musicvideo_core::{
    Fill, Glow, GradientStop, GradientStops, ImageId, Layer, LayerId, LayerKind, LyricLine,
    LyricsLayer, MIV_SPECTRUM_BANDS, ModulatorCalibration, ModulatorValue, MusicColorMode, Project,
    Rgba8, SpectrumDirection, SpectrumLayer, SpectrumShape, TextAlign, TextLayer, TimeFormat,
    TimeTextLayer, new_linear_spectrum_layer, new_lyrics_layer, new_miv_spectrum_keyboard_layers,
    new_miv_spectrum_layer, new_pitch_keyboard_layer, new_progress_bar_layer,
    new_radial_spectrum_layer, new_solid_layer, new_text_layer, new_time_text_layer,
};

use history::{EditSurface, UndoHistory};
use lyrics_editing::{
    EmptyTextMeaning, LyricsEditConflict, LyricsEditConflictKind, LyricsIdentitySnapshot,
    LyricsPassResult, LyricsResolutionChoice, LyricsTextBuffer, PendingLyricsInvalidation,
};
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
    lyrics_identities: HashMap<LayerId, LyricsIdentitySnapshot>,
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

pub struct EditorState {
    selected_layer: Option<LayerId>,
    selected_detail: Option<DetailSelection>,
    dragging_detail: Option<DetailSelection>,
    resources: ResourceManifest,
    target_lifetimes: EditTargetLifetimes,
    lyrics_notice: Option<(LayerId, String)>,
    lyrics_editors: HashMap<LayerId, LyricsTextBuffer>,
    pending_text_edit_restores: Vec<(egui::Id, PendingLyricsInvalidation)>,
    pending_layer_delete: Option<LayerId>,
    inspector_tab: InspectorTab,
    show_selection_bounds: bool,
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
            lyrics_editors: HashMap::new(),
            pending_text_edit_restores: Vec::new(),
            pending_layer_delete: None,
            inspector_tab: InspectorTab::Layer,
            show_selection_bounds: true,
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
        self.lyrics_editors.clear();
        self.pending_text_edit_restores.clear();
        for layer in &project.layers {
            if let LayerKind::Lyrics(lyrics) = &layer.kind {
                self.lyrics_editors
                    .insert(layer.id, LyricsTextBuffer::new_session(&lyrics.lines));
            }
        }
        self.pending_layer_delete = None;
        self.modulator_editor = ModulatorEditorState::default();
        self.history.clear();
    }

    /// Reconciles state after a caller may have changed the Project without an
    /// egui Context. Native lyrics history is treated conservatively because
    /// this entry point cannot observe TextEdit's current undo graph.
    pub fn reconcile(&mut self, project: &Project) {
        self.forget_native_lyrics_history_reachability();
        self.reconcile_preserving_native_reachability(project);
    }

    fn reconcile_preserving_native_reachability(&mut self, project: &Project) {
        self.target_lifetimes.reconcile(project);
        self.reconcile_lyrics_editors(project);
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

    fn reconcile_lyrics_editors(&mut self, project: &Project) {
        let removed = self
            .lyrics_editors
            .keys()
            .copied()
            .filter(|id| {
                !project
                    .layers
                    .iter()
                    .any(|layer| layer.id == *id && matches!(&layer.kind, LayerKind::Lyrics(_)))
            })
            .collect::<Vec<_>>();
        for id in removed {
            if let Some(mut editor) = self.lyrics_editors.remove(&id) {
                let widget_id = editor.widget_id(id);
                if let Some(invalidation) = editor.cancel_pending() {
                    self.pending_text_edit_restores
                        .push((widget_id, invalidation));
                }
            }
        }

        for layer in &project.layers {
            let LayerKind::Lyrics(lyrics) = &layer.kind else {
                continue;
            };
            let invalidation = match self.lyrics_editors.get_mut(&layer.id) {
                Some(editor) => {
                    if editor.sync_times(&lyrics.lines).is_err() {
                        let widget_id = editor.widget_id(layer.id);
                        editor
                            .replace_external(&lyrics.lines)
                            .map(|invalidation| (widget_id, invalidation))
                    } else {
                        None
                    }
                }
                None => {
                    self.lyrics_editors
                        .insert(layer.id, LyricsTextBuffer::new_session(&lyrics.lines));
                    None
                }
            };
            if let Some(invalidation) = invalidation {
                self.pending_text_edit_restores.push(invalidation);
            }
        }
    }

    fn restore_pending_text_edit_states(&mut self, context: &egui::Context) {
        for (id, invalidation) in self.pending_text_edit_restores.drain(..) {
            invalidation.restore_text_edit(context, id);
        }
    }

    fn refresh_native_lyrics_history(&mut self, context: &egui::Context) {
        for (layer, editor) in &mut self.lyrics_editors {
            editor.refresh_native_history_reachability(context, editor.widget_id(*layer));
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
            lyrics_identities: self
                .lyrics_editors
                .iter()
                .filter(|(id, _)| {
                    project.layers.iter().any(|layer| {
                        layer.id == **id && matches!(&layer.kind, LayerKind::Lyrics(_))
                    })
                })
                .map(|(id, editor)| (*id, editor.identity_snapshot()))
                .collect(),
            selected_layer: self.selected_layer,
            selected_detail: self.selected_detail,
        }
    }

    fn restore_snapshot(&mut self, project: &mut Project, snapshot: EditorSnapshot) {
        let EditorSnapshot {
            project: restored_project,
            resources,
            lyrics_identities,
            selected_layer,
            selected_detail,
        } = snapshot;
        *project = restored_project;
        self.resources = resources;

        let restored_lyrics = project
            .layers
            .iter()
            .filter_map(|layer| match &layer.kind {
                LayerKind::Lyrics(lyrics) => Some((layer.id, lyrics.lines.clone())),
                _ => None,
            })
            .collect::<Vec<_>>();
        let restored_ids = restored_lyrics
            .iter()
            .map(|(id, _)| *id)
            .collect::<std::collections::HashSet<_>>();
        let removed = self
            .lyrics_editors
            .keys()
            .copied()
            .filter(|id| !restored_ids.contains(id))
            .collect::<Vec<_>>();
        for id in removed {
            if let Some(mut editor) = self.lyrics_editors.remove(&id) {
                let widget_id = editor.widget_id(id);
                if let Some(invalidation) = editor.cancel_pending() {
                    self.pending_text_edit_restores
                        .push((widget_id, invalidation));
                }
            }
        }
        for (id, lines) in restored_lyrics {
            let identity = lyrics_identities.get(&id);
            let invalidation = match self.lyrics_editors.get_mut(&id) {
                Some(editor) => {
                    let widget_id = editor.widget_id(id);
                    let invalidation = match identity {
                        Some(identity) => editor
                            .restore_global_snapshot(&lines, identity)
                            .ok()
                            .flatten(),
                        None => editor.replace_external(&lines),
                    };
                    invalidation.map(|invalidation| (widget_id, invalidation))
                }
                None => {
                    let editor = identity
                        .and_then(|identity| {
                            LyricsTextBuffer::from_identity_snapshot_session(&lines, identity).ok()
                        })
                        .unwrap_or_else(|| LyricsTextBuffer::new_session(&lines));
                    self.lyrics_editors.insert(id, editor);
                    None
                }
            };
            if let Some(invalidation) = invalidation {
                self.pending_text_edit_restores.push(invalidation);
            }
        }

        self.selected_layer = selected_layer
            .filter(|id| project.layers.iter().any(|layer| layer.id == *id))
            .or_else(|| {
                project
                    .layers
                    .get(1)
                    .or_else(|| project.layers.first())
                    .map(|layer| layer.id)
            });
        self.selected_detail = selected_detail;
        self.dragging_detail = None;
        self.pending_layer_delete = None;
        self.modulator_editor = ModulatorEditorState::default();
        self.reconcile_preserving_native_reachability(project);
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

    pub fn has_pending_lyrics_edit(&self) -> bool {
        self.lyrics_editors
            .values()
            .any(LyricsTextBuffer::has_pending_resolution)
    }

    /// Present the existing lyrics decision before the host replaces/closes a document.
    /// Walk project order so unselected layers are resolved too; the buffers remain owners.
    pub fn pending_lyrics_confirmation(
        &mut self,
        context: &egui::Context,
        project: &mut Project,
    ) -> EditorResponse {
        let pending = project.layers.iter().find_map(|layer| {
            self.lyrics_editors
                .get(&layer.id)
                .and_then(LyricsTextBuffer::pending_conflict)
                .map(|conflict| (layer.id, layer.name.clone(), conflict))
        });
        let mut response = EditorResponse::default();
        if let Some((id, name, conflict)) = pending {
            let mut action = None;
            egui::Modal::new(egui::Id::new("musicvideo-pending-lyrics")).show(context, |ui| {
                ui.heading(name);
                action = draw_lyrics_resolution(ui, &conflict);
            });
            if let Some(action) = action {
                response = self.apply_pending_lyrics_confirmation(context, project, id, action);
            }
        }
        response
    }

    fn apply_pending_lyrics_confirmation(
        &mut self,
        context: &egui::Context,
        project: &mut Project,
        id: LayerId,
        action: LyricsConflictAction,
    ) -> EditorResponse {
        let checkpoint = self.begin_external_edit(project);
        if let Some(layer) = project.layers.iter_mut().find(|layer| layer.id == id)
            && let LayerKind::Lyrics(lyrics) = &mut layer.kind
        {
            apply_lyrics_conflict_action(self, context, id, lyrics, action);
        }
        EditorResponse {
            project_changed: self.finish_external_edit(project, checkpoint),
            ..Default::default()
        }
    }

    pub fn layer_has_pending_lyrics_edit(&self, layer: LayerId) -> bool {
        self.lyrics_editors
            .get(&layer)
            .is_some_and(LyricsTextBuffer::has_pending_resolution)
    }

    pub fn undo(&mut self, project: &mut Project) -> bool {
        self.forget_native_lyrics_history_reachability();
        self.undo_preserving_native_reachability(project)
    }

    fn undo_preserving_native_reachability(&mut self, project: &mut Project) -> bool {
        self.reconcile_preserving_native_reachability(project);
        let current = self.snapshot(project);
        let Some(snapshot) = self.history.undo(current) else {
            return false;
        };
        self.restore_snapshot(project, snapshot);
        true
    }

    pub fn redo(&mut self, project: &mut Project) -> bool {
        self.forget_native_lyrics_history_reachability();
        self.redo_preserving_native_reachability(project)
    }

    fn redo_preserving_native_reachability(&mut self, project: &mut Project) -> bool {
        self.reconcile_preserving_native_reachability(project);
        let current = self.snapshot(project);
        let Some(snapshot) = self.history.redo(current) else {
            return false;
        };
        self.restore_snapshot(project, snapshot);
        true
    }

    fn undo_in_context(&mut self, context: &egui::Context, project: &mut Project) -> bool {
        self.refresh_native_lyrics_history(context);
        let changed = self.undo_preserving_native_reachability(project);
        self.restore_pending_text_edit_states(context);
        changed
    }

    fn redo_in_context(&mut self, context: &egui::Context, project: &mut Project) -> bool {
        self.refresh_native_lyrics_history(context);
        let changed = self.redo_preserving_native_reachability(project);
        self.restore_pending_text_edit_states(context);
        changed
    }

    pub fn begin_external_edit(&mut self, project: &Project) -> EditCheckpoint {
        self.reconcile_preserving_native_reachability(project);
        EditCheckpoint(self.snapshot(project))
    }

    pub fn finish_external_edit(&mut self, project: &Project, checkpoint: EditCheckpoint) -> bool {
        // This entry point cannot inspect egui's current native TextEdit undo
        // graph. Do not let reconcile treat an older reachability observation
        // as authoritative if the external edit changed lyrics.
        self.forget_native_lyrics_history_reachability();
        self.finish_external_edit_preserving_native_reachability(project, checkpoint)
    }

    fn finish_external_edit_preserving_native_reachability(
        &mut self,
        project: &Project,
        checkpoint: EditCheckpoint,
    ) -> bool {
        self.reconcile_preserving_native_reachability(project);
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
        // See finish_external_edit: no Context means native reachability must
        // become conservative before an authoritative Project reconciliation.
        self.forget_native_lyrics_history_reachability();
        self.reconcile_preserving_native_reachability(project);
        resources.retain_project_resources(project);
        self.resources = resources;
        let after = self.snapshot(project);
        let changed = checkpoint.0 != after;
        self.history.record_atomic(checkpoint.0, &after);
        changed
    }

    fn forget_native_lyrics_history_reachability(&mut self) {
        for editor in self.lyrics_editors.values_mut() {
            editor.forget_native_history_reachability();
        }
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
        self.try_apply_external_command_inner(None, project, command)
    }

    pub fn try_apply_external_command_with_context(
        &mut self,
        context: &egui::Context,
        project: &mut Project,
        command: &EditorCommand,
    ) -> Result<bool, EditorCommandError> {
        self.try_apply_external_command_inner(Some(context), project, command)
    }

    fn try_apply_external_command_inner(
        &mut self,
        context: Option<&egui::Context>,
        project: &mut Project,
        command: &EditorCommand,
    ) -> Result<bool, EditorCommandError> {
        let checkpoint = self.begin_external_edit(project);
        if matches!(command, EditorCommand::ReplaceLyrics { .. }) {
            if let Some(context) = context {
                self.restore_pending_text_edit_states(context);
                self.refresh_native_lyrics_history(context);
            } else {
                self.forget_native_lyrics_history_reachability();
            }
        }
        let changed = self.try_apply_command(project, command)?;
        if changed {
            if context.is_some() {
                self.finish_external_edit_preserving_native_reachability(project, checkpoint);
            } else {
                self.finish_external_edit(project, checkpoint);
            }
        }
        if let Some(context) = context {
            self.restore_pending_text_edit_states(context);
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
        if let EditorCommand::ReplaceLyrics { layer, lines } = command {
            let invalidation = match self.lyrics_editors.get_mut(layer) {
                Some(editor) => {
                    let widget_id = editor.widget_id(*layer);
                    editor
                        .replace_external(lines)
                        .map(|invalidation| (widget_id, invalidation))
                }
                None => {
                    self.lyrics_editors
                        .insert(*layer, LyricsTextBuffer::new_session(lines));
                    None
                }
            };
            if let Some(invalidation) = invalidation {
                self.pending_text_edit_restores.push(invalidation);
            }
        }
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
            self.reconcile_preserving_native_reachability(project);
        }
        if matches!(command, EditorCommand::ReplaceLyrics { .. })
            && matches!(self.selected_detail, Some(DetailSelection::Lyric { .. }))
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

/// UI-only inspector selection, excluded from saving and Undo snapshots.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InspectorTab {
    #[default]
    Layer,
    Modulators,
    Project,
}

impl EditorState {
    pub fn preview_controls(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(&mut self.show_selection_bounds, "選択レイヤーの枠");
    }

    pub fn inspector_tab(&self) -> InspectorTab {
        self.inspector_tab
    }
    pub fn set_inspector_tab(&mut self, tab: InspectorTab) {
        self.inspector_tab = tab;
    }

    /// Prepare once before any timeline or properties input.
    /// Typed phases keep timeline edits outside the Properties checkpoint.
    pub fn begin_frame<'a>(
        &'a mut self,
        context: &egui::Context,
        project: &'a mut Project,
    ) -> PreparedEditorFrame<'a> {
        self.begin_frame_with_input_enabled(context, project, true)
    }

    /// Hosts render the regions in disabled Uis while presenting a decision modal.
    /// This flag also blocks context-level shortcuts and deletion confirmation.
    /// Reconciliation and history observation still run.
    pub fn begin_frame_with_input_enabled<'a>(
        &'a mut self,
        context: &egui::Context,
        project: &'a mut Project,
        input_enabled: bool,
    ) -> PreparedEditorFrame<'a> {
        self.restore_pending_text_edit_states(context);
        self.reconcile_preserving_native_reachability(project);
        let history_changed = input_enabled && apply_history_shortcut(context, project, self);
        PreparedEditorFrame {
            context: context.clone(),
            project,
            state: self,
            history_changed,
            input_enabled,
        }
    }
}

/// Pre-properties phase: restoration/shortcut has run. Draw history controls and
/// the full-width timeline, then consume this phase with `properties`.
#[must_use]
pub struct PreparedEditorFrame<'a> {
    context: egui::Context,
    project: &'a mut Project,
    state: &'a mut EditorState,
    history_changed: bool,
    input_enabled: bool,
}

impl<'a> PreparedEditorFrame<'a> {
    pub fn preview_controls(&mut self, ui: &mut egui::Ui) {
        self.state.preview_controls(ui);
    }

    pub fn history_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.state.can_undo(), egui::Button::new("元に戻す"))
                .on_hover_text("Ctrl+Z")
                .clicked()
            {
                self.history_changed |= self.state.undo_in_context(&self.context, self.project);
            }
            if ui
                .add_enabled(self.state.can_redo(), egui::Button::new("やり直す"))
                .on_hover_text("Ctrl+Y / Ctrl+Shift+Z")
                .clicked()
            {
                self.history_changed |= self.state.redo_in_context(&self.context, self.project);
            }
        });
    }
    pub fn timeline(
        &mut self,
        ui: &mut egui::Ui,
        position_secs: f64,
        waveform: Option<&TimelineWaveform>,
    ) -> TimelineResponse {
        draw_timeline(ui, self.project, self.state, position_secs, waveform)
    }
    /// Start the single transaction shared by the list and all inspector tabs.
    pub fn properties(self) -> EditorFrame<'a> {
        let checkpoint = self.state.begin_external_edit(self.project);
        EditorFrame {
            context: self.context,
            project: self.project,
            state: self.state,
            checkpoint,
            input_enabled: self.input_enabled,
            response: EditorResponse {
                history_changed: self.history_changed,
                ..Default::default()
            },
        }
    }
}

/// One Properties transaction. Regions only draw and accumulate edits. Consuming
/// `finish` performs deletion confirmation and exactly one history observation.
#[must_use]
pub struct EditorFrame<'a> {
    context: egui::Context,
    project: &'a mut Project,
    state: &'a mut EditorState,
    checkpoint: EditCheckpoint,
    input_enabled: bool,
    response: EditorResponse,
}

impl EditorFrame<'_> {
    pub fn layer_list(&mut self, ui: &mut egui::Ui) {
        panel_scroll(ui, "musicvideo-layer-list", |ui| {
            let project = &mut *self.project;
            let state = &mut *self.state;
            let mut project_changed = false;
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
            ui.vertical(|ui| {
                let can_move_toward_back = selected_layer.is_some() && selected_index > 0;
                if ui
                    .add_enabled(can_move_toward_back, egui::Button::new("一覧の上へ（奥）"))
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
                state.inspector_tab = InspectorTab::Layer;
                project_changed = true;
            }

            self.response.project_changed |= project_changed;
            self.response.add_image_layer_requested |= add_image_layer_requested;
        });
    }
    pub fn inspector(
        &mut self,
        ui: &mut egui::Ui,
        current_time_secs: f64,
        modulator_values: &[ModulatorValue],
        calibrations: &[ModulatorCalibration],
    ) {
        self.inspector_with_project_path(
            ui,
            current_time_secs,
            modulator_values,
            calibrations,
            None,
        );
    }

    pub fn inspector_with_project_path(
        &mut self,
        ui: &mut egui::Ui,
        current_time_secs: f64,
        modulator_values: &[ModulatorValue],
        calibrations: &[ModulatorCalibration],
        project_path: Option<&Path>,
    ) {
        ui.horizontal(|ui| {
            ui.selectable_value(
                &mut self.state.inspector_tab,
                InspectorTab::Layer,
                "レイヤー",
            );
            ui.selectable_value(
                &mut self.state.inspector_tab,
                InspectorTab::Modulators,
                "モジュレータ",
            );
            ui.selectable_value(
                &mut self.state.inspector_tab,
                InspectorTab::Project,
                "プロジェクト",
            );
        });
        ui.separator();
        let tab = self.state.inspector_tab;
        panel_scroll(ui, ("musicvideo-inspector", tab as u8), |ui| {
            let project = &mut *self.project;
            let state = &mut *self.state;
            match tab {
                InspectorTab::Layer => {
                    let mut project_changed = false;
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

                    self.response.project_changed |= project_changed;
                    self.response.import_lrc_requested = import_lrc_requested;
                    self.response.export_lrc_requested = export_lrc_requested;
                }
                InspectorTab::Modulators => {
                    self.response.project_changed |= modulators::draw_modulators(
                        ui,
                        project,
                        &mut state.modulator_editor,
                        modulator_values,
                        calibrations,
                    );
                }
                InspectorTab::Project => {
                    ui.heading("出力");
                    ui.label(format!(
                        "解像度: {} × {}",
                        project.output.width, project.output.height
                    ));
                    let fps_before = project.output.fps;
                    let fps_label = if [24.0, 30.0, 60.0].contains(&fps_before) {
                        format!("{fps_before} fps")
                    } else {
                        format!("{fps_before} fps（このプロジェクトの値）")
                    };
                    egui::ComboBox::from_id_salt("output-fps")
                        .selected_text(fps_label)
                        .show_ui(ui, |ui| {
                            for fps in [24.0, 30.0, 60.0] {
                                ui.selectable_value(
                                    &mut project.output.fps,
                                    fps,
                                    format!("{fps} fps"),
                                );
                            }
                        });
                    self.response.project_changed |= project.output.fps != fps_before;
                    ui.separator();
                    self.response.project_changed |= ui
                        .add(
                            egui::DragValue::new(&mut project.display_gain_db)
                                .range(-48.0..=48.0)
                                .speed(0.1)
                                .prefix("表示ゲイン ")
                                .suffix(" dB"),
                        )
                        .on_hover_text("鍵盤の点灯強度だけを調整します。音声の音量は変わりません")
                        .changed();
                    draw_project_paths(ui, project, project_path);
                }
            }
        });
    }
    pub fn finish(mut self) -> EditorResponse {
        if self.input_enabled
            && let Some(command) =
                draw_layer_delete_confirmation(&self.context, self.project, self.state)
        {
            self.response.project_changed |= self.state.apply_command(self.project, &command);
        }
        // Direct UI additions must have lifetimes before the host captures a preview.
        // Reconcile only targets here, preserving lyrics' native Undo reachability.
        self.state.target_lifetimes.reconcile(self.project);
        let pointer_active = self.context.input(|input| input.pointer.primary_down());
        let text_focus = self.context.memory(|memory| memory.focused());
        self.state.observe_ui_pass(
            EditSurface::Properties,
            self.project,
            self.checkpoint,
            self.response.project_changed,
            pointer_active,
            text_focus,
        );
        self.response.project_changed |= self.response.history_changed;
        self.response
    }
}

fn panel_scroll(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    draw: impl FnOnce(&mut egui::Ui),
) {
    let id = egui::Id::new(id);
    let size = ui.available_size().max(egui::Vec2::ZERO);
    ui.allocate_ui_with_layout(size, egui::Layout::top_down(egui::Align::LEFT), |ui| {
        ui.set_min_size(size);
        ui.set_max_width(size.x);
        ui.style_mut().spacing.scroll = egui::style::ScrollStyle::solid();
        egui::ScrollArea::vertical()
            .id_salt(id)
            .max_height(size.y)
            .min_scrolled_height(0.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // ScrollArea's salt identifies its scroll state, not its child
                // Ui. Scope every region/tab's widgets too, so TextEdit native
                // state cannot be reused by another tab's DragValue.
                ui.push_id(id, draw);
            });
    });
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
    let text_input_owned = context.egui_wants_keyboard_input()
        || egui_ime::ime_input_active(context)
        || egui_ime::recovering_text_input(context).is_some();
    let requested = context.input(|input| {
        input
            .events
            .iter()
            .find_map(|event| history_action_for_event(event, text_input_owned))
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
        HistoryAction::Undo => state.undo_in_context(context, project),
        HistoryAction::Redo => state.redo_in_context(context, project),
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

/// Read-only project/material paths belong to the scrollable project inspector.
pub fn draw_project_paths(ui: &mut egui::Ui, project: &Project, project_path: Option<&Path>) {
    ui.separator();
    ui.label(format!(
        "プロジェクト: {}",
        project_path.map_or_else(|| "新規".to_owned(), |p| p.display().to_string())
    ));
    if let Some(audio) = &project.audio {
        ui.label(format!("音声: {}", audio.path.display()));
    }
    for image in &project.images {
        ui.label(format!("画像 {}: {}", image.id.0, image.path.display()));
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
    let input_enabled = ui.is_enabled();
    let mut changed = egui_ime::add_singleline(ui, &mut layer.name, None, |edit| {
        edit.interactive(input_enabled)
    })
    .changed();
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
        (AnimProperty::TextStyle, AnimationValue::TextStyle(style)) => text_style::edit(ui, style),
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
    let input_enabled = ui.is_enabled();
    let mut changed = egui_ime::add_singleline(ui, &mut text.text, None, |edit| {
        edit.interactive(input_enabled)
    })
    .changed();
    changed |= edit_text_layout(ui, &mut text.size_px, &mut text.align);
    changed
}

#[derive(Default)]
struct LyricsEditResult {
    changed: bool,
    import_requested: bool,
    export_requested: bool,
}

enum LyricsConflictAction {
    Resolve(LyricsResolutionChoice),
    Cancel,
}

fn apply_lyrics_conflict_action(
    state: &mut EditorState,
    context: &egui::Context,
    layer_id: LayerId,
    lyrics: &mut musicvideo_core::LyricsLayer,
    action: LyricsConflictAction,
) -> bool {
    let editor = state
        .lyrics_editors
        .get_mut(&layer_id)
        .expect("pending lyrics editor exists");
    match action {
        LyricsConflictAction::Resolve(choice) => {
            if let Ok(LyricsPassResult::Applied(lines)) =
                editor.resolve_pending(&lyrics.lines, choice)
            {
                lyrics.lines = lines;
                if matches!(state.selected_detail, Some(DetailSelection::Lyric { .. })) {
                    state.clear_detail_selection();
                }
                return true;
            }
        }
        LyricsConflictAction::Cancel => {
            let widget_id = editor.widget_id(layer_id);
            if let Some(invalidation) = editor.cancel_pending() {
                invalidation.restore_text_edit(context, widget_id);
            }
        }
    }
    false
}

fn draw_lyrics_resolution(
    ui: &mut egui::Ui,
    conflict: &LyricsEditConflict,
) -> Option<LyricsConflictAction> {
    let mut action = None;
    ui.group(|ui| {
        ui.set_max_width(ui.available_width());
        ui.colored_label(egui::Color32::YELLOW, "歌詞の行と時刻を確認してください");
        egui::Grid::new("lyrics-edit-comparison")
            .num_columns(2)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                ui.strong("変更前");
                ui.strong("変更後");
                ui.end_row();
                lyrics_preview(ui, "lyrics-before", &format_lyrics_lines(&conflict.before));
                lyrics_preview(
                    ui,
                    "lyrics-after",
                    &format_lyrics_text(&conflict.after_text),
                );
                ui.end_row();
            });

        match &conflict.kind {
            LyricsEditConflictKind::TimedLineMerge { choices } => {
                ui.label("複数の時刻付き行が1行になります。どの行の時刻を残すか選んでください。");
                for choice in choices {
                    let text = if choice.source_text.is_empty() {
                        "（空行）"
                    } else {
                        &choice.source_text
                    };
                    if ui
                        .button(format!("{:.3} 秒 — {text}", choice.time_secs))
                        .clicked()
                    {
                        action = Some(LyricsConflictAction::Resolve(
                            LyricsResolutionChoice::KeepTimedLine(choice.line_id),
                        ));
                    }
                }
            }
            LyricsEditConflictKind::AmbiguousNativeHistory { choices } => {
                ui.label("以前の行との対応が複数あります。残す時刻を見比べて選んでください。");
                egui::ScrollArea::vertical()
                    .id_salt("lyrics-native-candidates")
                    .max_height(150.0)
                    .show(ui, |ui| {
                        for (index, choice) in choices.iter().enumerate() {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(format!(
                                    "候補 {}: {}",
                                    index + 1,
                                    format_lyrics_lines(&choice.lines)
                                ));
                                if ui.small_button("この対応を使う").clicked() {
                                    action = Some(LyricsConflictAction::Resolve(
                                        LyricsResolutionChoice::UseNativeCandidate(
                                            choice.candidate_id,
                                        ),
                                    ));
                                }
                            });
                        }
                    });
            }
            LyricsEditConflictKind::UnrecordedNativeHistory { target_is_empty } => {
                ui.label(
                    "以前の行との対応を確定できません。反映する場合は時刻が未設定になります。",
                );
                if *target_is_empty {
                    if ui.button("行なしとして反映（時刻は未設定）").clicked() {
                        action = Some(LyricsConflictAction::Resolve(
                            LyricsResolutionChoice::ApplyAsNewUntimed(EmptyTextMeaning::NoLines),
                        ));
                    }
                    if ui.button("空の1行として反映（時刻は未設定）").clicked() {
                        action = Some(LyricsConflictAction::Resolve(
                            LyricsResolutionChoice::ApplyAsNewUntimed(
                                EmptyTextMeaning::OneUntimedLine,
                            ),
                        ));
                    }
                } else if ui.button("新規歌詞として反映（時刻は未設定）").clicked()
                {
                    action = Some(LyricsConflictAction::Resolve(
                        LyricsResolutionChoice::ApplyAsNewUntimed(EmptyTextMeaning::OneUntimedLine),
                    ));
                }
            }
        }
        if ui.button("変更を取り消す").clicked() {
            action = Some(LyricsConflictAction::Cancel);
        }
    });
    action
}

fn lyrics_preview(ui: &mut egui::Ui, salt: &str, text: &str) {
    egui::ScrollArea::vertical()
        .id_salt(salt)
        .max_height(100.0)
        .show(ui, |ui| {
            ui.set_max_width((ui.available_width() / 2.0).max(80.0));
            ui.add(egui::Label::new(text).wrap());
        });
}

fn format_lyrics_text(text: &str) -> String {
    if text.is_empty() {
        "（空の本文）".to_owned()
    } else {
        text.to_owned()
    }
}

fn format_lyrics_lines(lines: &[LyricLine]) -> String {
    if lines.is_empty() {
        return "（行なし）".to_owned();
    }
    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let text = if line.text.is_empty() {
                "（空行）"
            } else {
                &line.text
            };
            let time = line
                .time_secs
                .map(|time| format!("{time:.3} 秒"))
                .unwrap_or_else(|| "未設定".to_owned());
            format!("{}: {text} [{time}]", index + 1)
        })
        .collect::<Vec<_>>()
        .join("\n")
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

    let input_enabled = ui.is_enabled();
    let mut invalidation = None;
    let pass_result = {
        let editor = state
            .lyrics_editors
            .entry(layer_id)
            .or_insert_with(|| LyricsTextBuffer::new_session(&lyrics.lines));
        let widget_id = editor.widget_id(layer_id);
        if editor.sync_times(&lyrics.lines).is_err() {
            invalidation = editor
                .replace_external(&lyrics.lines)
                .map(|invalidation| (widget_id, invalidation));
        }
        let rollback = editor.capture_text_edit_rollback_if_needed(ui.ctx(), widget_id);
        let may_mutate = rollback.is_some();
        editor
            .begin_pass(&lyrics.lines, rollback)
            .expect("lyrics editor was synchronized before the text pass");
        ui.add(
            egui::TextEdit::multiline(editor)
                .interactive(input_enabled)
                .id(widget_id)
                .desired_rows(7)
                .desired_width(f32::INFINITY)
                .hint_text("ここに歌詞をまとめて貼り付け"),
        );
        if may_mutate {
            editor.refresh_native_history_reachability(ui.ctx(), widget_id);
        }
        let mut pass_result = editor.finish_pass();
        if editor.has_pending_resolution()
            && let Ok(refreshed) = editor.refresh_pending(&lyrics.lines)
        {
            pass_result = refreshed;
        }
        pass_result
    };
    if let Some(invalidation) = invalidation {
        state.pending_text_edit_restores.push(invalidation);
    }
    match pass_result {
        LyricsPassResult::Unchanged => {}
        LyricsPassResult::Applied(lines) => {
            lyrics.lines = lines;
            result.changed = true;
            if matches!(state.selected_detail, Some(DetailSelection::Lyric { .. })) {
                state.clear_detail_selection();
            }
        }
        LyricsPassResult::NeedsResolution(_) => {}
    }

    if let Some(conflict) = state
        .lyrics_editors
        .get(&layer_id)
        .and_then(LyricsTextBuffer::pending_conflict)
        && let Some(action) = draw_lyrics_resolution(ui, &conflict)
    {
        result.changed |= apply_lyrics_conflict_action(state, ui.ctx(), layer_id, lyrics, action);
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

// Color32 stores premultiplied channels; the project/rasterizer uses straight RGBA.
fn color_from_egui(value: egui::Color32) -> Rgba8 {
    let [r, g, b, a] = value.to_srgba_unmultiplied();
    Rgba8::new(r, g, b, a)
}

fn edit_color(ui: &mut egui::Ui, label: &str, color: &mut Rgba8) -> bool {
    let mut value = egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a);
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        changed = ui.color_edit_button_srgba(&mut value).changed();
    });
    if changed {
        *color = color_from_egui(value);
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
    use egui::TextBuffer;
    use egui::text::{CCursor, CCursorRange};
    use egui::text_edit::TextEditState;
    use musicvideo_core::{
        Animatable, AudioRef, ImageRef, Interp, Keyframe, LyricLine, ModulationBinding,
        ModulationCurve, ModulationTarget, Track, default_project, new_band_energy_modulator,
        new_image_layer, new_solid_layer,
    };
    use std::sync::Arc;

    fn pending_merge_choice(state: &EditorState, id: LayerId) -> LyricsConflictAction {
        let conflict = state.lyrics_editors[&id].pending_conflict().unwrap();
        let LyricsEditConflictKind::TimedLineMerge { choices } = conflict.kind else {
            panic!("expected a timed merge");
        };
        LyricsConflictAction::Resolve(LyricsResolutionChoice::KeepTimedLine(choices[0].line_id))
    }

    #[test]
    fn p4b4_lyrics_handler_continues_needs_resolution_and_records_one_transaction() {
        let original = vec![
            lyric_line("A", Some(1.0)),
            lyric_line("B", Some(2.0)),
            lyric_line("C", Some(3.0)),
            lyric_line("D", Some(4.0)),
        ];
        let (mut project, id) = project_with_lyrics(original.clone());
        let before = project.clone();
        let mut state = EditorState::default();
        state.reset_for_project(&project);
        let editor = state.lyrics_editors.get_mut(&id).unwrap();
        editor.begin_pass(&original, None).unwrap();
        TextBuffer::delete_char_range(editor, egui::text::CharIndex(1)..egui::text::CharIndex(2));
        TextBuffer::delete_char_range(editor, egui::text::CharIndex(4)..egui::text::CharIndex(5));
        assert!(matches!(
            editor.finish_pass(),
            LyricsPassResult::NeedsResolution(_)
        ));
        let context = test_context();
        let choice = pending_merge_choice(&state, id);
        let response = state.apply_pending_lyrics_confirmation(&context, &mut project, id, choice);
        assert!(!response.project_changed);
        assert!(state.has_pending_lyrics_edit());
        assert_eq!(project, before);
        let choice = pending_merge_choice(&state, id);
        let response = state.apply_pending_lyrics_confirmation(&context, &mut project, id, choice);
        assert!(response.project_changed);
        assert!(!state.has_pending_lyrics_edit());
        assert_eq!(state.history_depths(), (1, 0));
        assert_eq!(
            lyrics_lines(&project, id),
            vec![lyric_line("AB", Some(1.0)), lyric_line("CD", Some(3.0))]
        );
        assert!(state.undo(&mut project));
        assert_eq!(project, before);
    }

    #[test]
    fn p4b4_lyrics_handler_unchanged_is_complete_without_history() {
        let original = vec![lyric_line("A", Some(1.0)), lyric_line("B", Some(2.0))];
        let (mut project, id) = project_with_lyrics(original.clone());
        let before = project.clone();
        let mut state = EditorState::default();
        state.reset_for_project(&project);
        let editor = state.lyrics_editors.get_mut(&id).unwrap();
        editor.begin_pass(&original, None).unwrap();
        TextBuffer::delete_char_range(editor, egui::text::CharIndex(1)..egui::text::CharIndex(2));
        TextBuffer::replace_with(editor, "A\nB");
        assert!(matches!(
            editor.finish_pass(),
            LyricsPassResult::NeedsResolution(_)
        ));
        let context = test_context();
        let choice = pending_merge_choice(&state, id);
        let response = state.apply_pending_lyrics_confirmation(&context, &mut project, id, choice);
        assert!(!response.project_changed);
        assert!(!state.has_pending_lyrics_edit());
        assert_eq!(project, before);
        assert_eq!(state.history_depths(), (0, 0));
    }

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

    fn project_with_lyrics(lines: Vec<LyricLine>) -> (Project, LayerId) {
        let mut project = default_project(10.0);
        let mut layer = new_lyrics_layer(&project);
        let layer_id = layer.id;
        let LayerKind::Lyrics(lyrics) = &mut layer.kind else {
            unreachable!()
        };
        lyrics.lines = lines;
        project.layers.push(layer);
        (project, layer_id)
    }

    fn lyric_line(text: &str, time_secs: Option<f64>) -> LyricLine {
        LyricLine {
            text: text.to_owned(),
            time_secs,
        }
    }

    fn lyrics_lines(project: &Project, layer_id: LayerId) -> &[LyricLine] {
        let layer = project
            .layers
            .iter()
            .find(|layer| layer.id == layer_id)
            .unwrap();
        let LayerKind::Lyrics(lyrics) = &layer.kind else {
            unreachable!()
        };
        &lyrics.lines
    }

    fn run_editor_frame(
        context: &egui::Context,
        project: &mut Project,
        state: &mut EditorState,
        time: f64,
        events: Vec<egui::Event>,
    ) {
        let _ = crate::headless_paint_output(context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 900.0),
                )),
                time: Some(time),
                events,
                ..Default::default()
            },
            |context| {
                let mut prepared = state.begin_frame(context, project);
                egui::Panel::bottom("test-timeline").show(context, |ui| {
                    prepared.timeline(ui, 0.0, None);
                });
                let mut frame = prepared.properties();
                egui::Panel::left("test-layers")
                    .default_size(220.0)
                    .show(context, |ui| {
                        frame.layer_list(ui);
                    });
                egui::Panel::right("test-inspector")
                    .default_size(360.0)
                    .show(context, |ui| {
                        frame.inspector(ui, 0.0, &[], &[]);
                    });
                frame.finish();
            },
        ));
    }

    fn set_text_cursor(context: &egui::Context, id: egui::Id, range: CCursorRange) {
        let mut text_state = TextEditState::load(context, id).unwrap_or_default();
        text_state.cursor.set_char_range(Some(range));
        text_state.store(context, id);
        context.memory_mut(|memory| memory.request_focus(id));
    }

    #[test]
    fn editor_frame_leading_paste_keeps_existing_lyric_times() {
        let (mut project, layer_id) = project_with_lyrics(vec![
            lyric_line("一行目", Some(1.0)),
            lyric_line("二行目", Some(2.0)),
        ]);
        let mut state = EditorState::default();
        state.reset_for_project(&project);
        state.select_layer_id(&project, layer_id);
        let context = test_context();
        run_editor_frame(&context, &mut project, &mut state, 0.0, Vec::new());
        let text_id = state.lyrics_editors[&layer_id].widget_id(layer_id);
        set_text_cursor(&context, text_id, CCursorRange::one(CCursor::new(0)));
        run_editor_frame(&context, &mut project, &mut state, 0.01, Vec::new());

        run_editor_frame(
            &context,
            &mut project,
            &mut state,
            0.1,
            vec![egui::Event::Text("先頭\n".to_owned())],
        );

        assert_eq!(
            lyrics_lines(&project, layer_id),
            [
                lyric_line("先頭", None),
                lyric_line("一行目", Some(1.0)),
                lyric_line("二行目", Some(2.0)),
            ]
        );
    }

    #[test]
    fn editor_frame_ime_replacement_keeps_the_timed_line_identity() {
        let (mut project, layer_id) = project_with_lyrics(vec![lyric_line("旧歌詞", Some(3.0))]);
        let mut state = EditorState::default();
        state.reset_for_project(&project);
        state.select_layer_id(&project, layer_id);
        let context = test_context();
        run_editor_frame(&context, &mut project, &mut state, 0.0, Vec::new());
        let text_id = state.lyrics_editors[&layer_id].widget_id(layer_id);
        let selection = CCursorRange::two(CCursor::new(0), CCursor::new(3));
        set_text_cursor(&context, text_id, selection);
        assert_eq!(
            TextEditState::load(&context, text_id)
                .unwrap()
                .cursor
                .char_range(),
            Some(selection)
        );
        run_editor_frame(&context, &mut project, &mut state, 0.01, Vec::new());
        assert_eq!(
            TextEditState::load(&context, text_id)
                .unwrap()
                .cursor
                .char_range(),
            Some(selection)
        );
        run_editor_frame(
            &context,
            &mut project,
            &mut state,
            0.02,
            vec![egui::Event::Ime(egui::ImeEvent::Enabled)],
        );
        run_editor_frame(
            &context,
            &mut project,
            &mut state,
            0.03,
            vec![
                egui::Event::Ime(egui::ImeEvent::Preedit {
                    text: "新歌詞".to_owned(),
                    active_range_chars: None,
                }),
                egui::Event::Ime(egui::ImeEvent::Commit("新歌詞".to_owned())),
            ],
        );

        assert_eq!(
            lyrics_lines(&project, layer_id),
            [lyric_line("新歌詞", Some(3.0))]
        );
    }

    #[test]
    fn editor_frame_native_undo_reaches_the_initial_checkpoint_after_many_input_passes() {
        let original = (0..180)
            .map(|index| lyric_line(&format!("歌詞 {index}"), Some(index as f64 * 0.5)))
            .collect::<Vec<_>>();
        let (mut project, layer_id) = project_with_lyrics(original.clone());
        let mut state = EditorState::default();
        state.reset_for_project(&project);
        state.select_layer_id(&project, layer_id);
        let context = test_context();
        run_editor_frame(&context, &mut project, &mut state, 0.0, Vec::new());
        let text_id = state.lyrics_editors[&layer_id].widget_id(layer_id);
        let initial_char_count = state.lyrics_editors[&layer_id].as_str().chars().count();
        set_text_cursor(
            &context,
            text_id,
            CCursorRange::one(CCursor::new(initial_char_count)),
        );
        run_editor_frame(&context, &mut project, &mut state, 0.001, Vec::new());

        for index in 0..320 {
            run_editor_frame(
                &context,
                &mut project,
                &mut state,
                0.002 + index as f64 * 0.001,
                vec![egui::Event::Text("あ".to_owned())],
            );
        }
        assert_eq!(lyrics_lines(&project, layer_id).len(), original.len());
        assert!(
            lyrics_lines(&project, layer_id)
                .iter()
                .zip(&original)
                .all(|(current, original)| current.time_secs == original.time_secs)
        );
        let native_state = TextEditState::load(&context, text_id).unwrap();
        let native_cursor = native_state.cursor.char_range().unwrap();
        let current_text = state.lyrics_editors[&layer_id].as_str().to_owned();
        assert_eq!(
            context.memory(|memory| memory.focused()),
            Some(text_id),
            "lyrics TextEdit must still own Ctrl+Z"
        );
        assert!(
            native_state
                .undoer()
                .has_undo(&(native_cursor, current_text))
        );

        // Idle passes should not deep-clone the native undoer. Keep drawing a
        // typical song long enough for egui's default stable-state checkpoint.
        for index in 0..200 {
            run_editor_frame(
                &context,
                &mut project,
                &mut state,
                0.5 + index as f64 * 0.01,
                Vec::new(),
            );
        }

        let modifiers = egui::Modifiers::CTRL | egui::Modifiers::COMMAND;
        run_editor_frame(
            &context,
            &mut project,
            &mut state,
            2.6,
            vec![egui::Event::Key {
                key: egui::Key::Z,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
        );

        assert_eq!(lyrics_lines(&project, layer_id), original);
    }

    #[test]
    fn cancelling_an_editor_frame_merge_restores_the_text_cursor_and_project() {
        let original = vec![lyric_line("A", Some(1.0)), lyric_line("B", Some(2.0))];
        let (mut project, layer_id) = project_with_lyrics(original.clone());
        let mut state = EditorState::default();
        state.reset_for_project(&project);
        state.select_layer_id(&project, layer_id);
        let context = test_context();
        run_editor_frame(&context, &mut project, &mut state, 0.0, Vec::new());
        let text_id = state.lyrics_editors[&layer_id].widget_id(layer_id);
        let selection = CCursorRange::two(CCursor::new(1), CCursor::new(2));
        set_text_cursor(&context, text_id, selection);
        run_editor_frame(&context, &mut project, &mut state, 0.01, Vec::new());

        run_editor_frame(
            &context,
            &mut project,
            &mut state,
            0.02,
            vec![egui::Event::Cut],
        );
        assert_eq!(lyrics_lines(&project, layer_id), original);
        assert!(state.lyrics_editors[&layer_id].has_pending_resolution());
        assert!(state.has_pending_lyrics_edit());
        assert!(state.layer_has_pending_lyrics_edit(layer_id));
        assert!(!state.layer_has_pending_lyrics_edit(project.layers[0].id));

        let invalidation = state
            .lyrics_editors
            .get_mut(&layer_id)
            .unwrap()
            .cancel_pending()
            .unwrap();
        invalidation.restore_text_edit(&context, text_id);
        let restored = TextEditState::load(&context, text_id).unwrap();
        assert_eq!(restored.cursor.char_range(), Some(selection));
        assert_eq!(state.lyrics_editors[&layer_id].as_str(), "A\nB");
        assert_eq!(lyrics_lines(&project, layer_id), original);
        assert!(!state.has_pending_lyrics_edit());
        assert!(!state.layer_has_pending_lyrics_edit(layer_id));

        run_editor_frame(
            &context,
            &mut project,
            &mut state,
            0.03,
            vec![egui::Event::Cut],
        );
        let keep = match state.lyrics_editors[&layer_id]
            .pending_conflict()
            .unwrap()
            .kind
        {
            LyricsEditConflictKind::TimedLineMerge { choices } => choices[0].line_id,
            _ => unreachable!(),
        };
        let resolved = state
            .lyrics_editors
            .get_mut(&layer_id)
            .unwrap()
            .resolve_pending(&original, LyricsResolutionChoice::KeepTimedLine(keep))
            .unwrap();
        let LyricsPassResult::Applied(lines) = resolved else {
            unreachable!()
        };
        let layer = project
            .layers
            .iter_mut()
            .find(|layer| layer.id == layer_id)
            .unwrap();
        let LayerKind::Lyrics(lyrics) = &mut layer.kind else {
            unreachable!()
        };
        lyrics.lines = lines;
        assert!(!state.has_pending_lyrics_edit());
        assert!(!state.layer_has_pending_lyrics_edit(layer_id));
    }

    #[test]
    fn native_text_undo_after_global_undo_keeps_the_line_time() {
        let (mut project, layer_id) = project_with_lyrics(vec![lyric_line("A", Some(6.0))]);
        let mut state = EditorState::default();
        state.reset_for_project(&project);
        state.select_layer_id(&project, layer_id);
        let context = test_context();
        run_editor_frame(&context, &mut project, &mut state, 0.0, Vec::new());
        let text_id = state.lyrics_editors[&layer_id].widget_id(layer_id);
        set_text_cursor(
            &context,
            text_id,
            CCursorRange::two(CCursor::new(0), CCursor::new(1)),
        );
        run_editor_frame(&context, &mut project, &mut state, 0.01, Vec::new());
        run_editor_frame(
            &context,
            &mut project,
            &mut state,
            0.02,
            vec![egui::Event::Text("B".to_owned())],
        );
        assert_eq!(
            lyrics_lines(&project, layer_id),
            [lyric_line("B", Some(6.0))]
        );
        run_editor_frame(&context, &mut project, &mut state, 2.0, Vec::new());
        context.memory_mut(|memory| memory.surrender_focus(text_id));
        run_editor_frame(&context, &mut project, &mut state, 2.1, Vec::new());

        assert!(state.undo_in_context(&context, &mut project));
        assert_eq!(
            lyrics_lines(&project, layer_id),
            [lyric_line("A", Some(6.0))]
        );

        set_text_cursor(&context, text_id, CCursorRange::one(CCursor::new(1)));
        run_editor_frame(&context, &mut project, &mut state, 2.2, Vec::new());
        let modifiers = egui::Modifiers::CTRL | egui::Modifiers::COMMAND;
        run_editor_frame(
            &context,
            &mut project,
            &mut state,
            2.3,
            vec![egui::Event::Key {
                key: egui::Key::Z,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
        );
        assert_eq!(
            lyrics_lines(&project, layer_id),
            [lyric_line("B", Some(6.0))]
        );
    }

    #[test]
    fn global_undo_redo_restores_lyrics_text_and_line_identity_together() {
        let original = vec![
            lyric_line("一行目", Some(1.0)),
            lyric_line("二行目", Some(2.0)),
        ];
        let (mut project, layer_id) = project_with_lyrics(original.clone());
        let mut state = EditorState::default();
        state.reset_for_project(&project);
        state.select_layer_id(&project, layer_id);
        let original_identity = state.lyrics_editors[&layer_id].identity_snapshot();
        let context = test_context();
        run_editor_frame(&context, &mut project, &mut state, 0.0, Vec::new());
        let text_id = state.lyrics_editors[&layer_id].widget_id(layer_id);
        set_text_cursor(&context, text_id, CCursorRange::one(CCursor::new(0)));
        run_editor_frame(&context, &mut project, &mut state, 0.01, Vec::new());
        run_editor_frame(
            &context,
            &mut project,
            &mut state,
            0.02,
            vec![egui::Event::Text("先頭\n".to_owned())],
        );
        let inserted_identity = state.lyrics_editors[&layer_id].identity_snapshot();
        assert_ne!(inserted_identity, original_identity);
        assert_eq!(
            lyrics_lines(&project, layer_id),
            [
                lyric_line("先頭", None),
                lyric_line("一行目", Some(1.0)),
                lyric_line("二行目", Some(2.0)),
            ]
        );

        assert!(state.undo_in_context(&context, &mut project));
        assert_eq!(lyrics_lines(&project, layer_id), original);
        assert_eq!(
            state.lyrics_editors[&layer_id].identity_snapshot(),
            original_identity
        );

        assert!(state.redo_in_context(&context, &mut project));
        assert_eq!(
            lyrics_lines(&project, layer_id),
            [
                lyric_line("先頭", None),
                lyric_line("一行目", Some(1.0)),
                lyric_line("二行目", Some(2.0)),
            ]
        );
        assert_eq!(
            state.lyrics_editors[&layer_id].identity_snapshot(),
            inserted_identity
        );
    }

    #[test]
    fn authoritative_same_lyrics_cancel_a_pending_draft_and_restore_native_state() {
        let original = vec![lyric_line("A", Some(1.0)), lyric_line("B", Some(2.0))];
        let (mut project, layer_id) = project_with_lyrics(original.clone());
        let mut state = EditorState::default();
        state.reset_for_project(&project);
        state.select_layer_id(&project, layer_id);
        let context = test_context();
        run_editor_frame(&context, &mut project, &mut state, 0.0, Vec::new());
        let text_id = state.lyrics_editors[&layer_id].widget_id(layer_id);
        let selection = CCursorRange::two(CCursor::new(1), CCursor::new(2));
        set_text_cursor(&context, text_id, selection);
        run_editor_frame(&context, &mut project, &mut state, 0.01, Vec::new());
        run_editor_frame(
            &context,
            &mut project,
            &mut state,
            0.02,
            vec![egui::Event::Cut],
        );
        assert!(state.layer_has_pending_lyrics_edit(layer_id));

        let changed = state
            .try_apply_external_command_with_context(
                &context,
                &mut project,
                &EditorCommand::ReplaceLyrics {
                    layer: layer_id,
                    lines: original.clone(),
                },
            )
            .unwrap();

        assert!(!changed);
        assert!(!state.layer_has_pending_lyrics_edit(layer_id));
        assert_eq!(lyrics_lines(&project, layer_id), original);
        assert_eq!(state.lyrics_editors[&layer_id].as_str(), "A\nB");
        assert_eq!(
            TextEditState::load(&context, text_id)
                .unwrap()
                .cursor
                .char_range(),
            Some(selection)
        );
    }

    #[test]
    fn editor_frame_layer_switch_keeps_each_lyrics_identity_separate() {
        let (mut project, first_id) = project_with_lyrics(vec![lyric_line("一", Some(1.0))]);
        let mut second = new_lyrics_layer(&project);
        let second_id = second.id;
        let LayerKind::Lyrics(second_lyrics) = &mut second.kind else {
            unreachable!()
        };
        second_lyrics.lines = vec![lyric_line("二", Some(2.0))];
        project.layers.push(second);
        let mut state = EditorState::default();
        state.reset_for_project(&project);
        let context = test_context();

        state.select_layer_id(&project, first_id);
        run_editor_frame(&context, &mut project, &mut state, 0.0, Vec::new());
        let first_text_id = state.lyrics_editors[&first_id].widget_id(first_id);
        set_text_cursor(&context, first_text_id, CCursorRange::one(CCursor::new(1)));
        run_editor_frame(&context, &mut project, &mut state, 0.01, Vec::new());
        run_editor_frame(
            &context,
            &mut project,
            &mut state,
            0.02,
            vec![egui::Event::Text("甲".to_owned())],
        );

        state.select_layer_id(&project, second_id);
        run_editor_frame(&context, &mut project, &mut state, 0.03, Vec::new());
        let second_text_id = state.lyrics_editors[&second_id].widget_id(second_id);
        set_text_cursor(&context, second_text_id, CCursorRange::one(CCursor::new(1)));
        run_editor_frame(&context, &mut project, &mut state, 0.04, Vec::new());
        run_editor_frame(
            &context,
            &mut project,
            &mut state,
            0.05,
            vec![egui::Event::Text("乙".to_owned())],
        );

        assert_eq!(
            lyrics_lines(&project, first_id),
            [lyric_line("一甲", Some(1.0))]
        );
        assert_eq!(
            lyrics_lines(&project, second_id),
            [lyric_line("二乙", Some(2.0))]
        );
        assert_ne!(first_text_id, second_text_id);
    }

    #[test]
    fn resetting_the_project_changes_lyrics_widget_lifetime() {
        let (project, layer_id) = project_with_lyrics(vec![lyric_line("A", Some(1.0))]);
        let mut state = EditorState::default();
        state.reset_for_project(&project);
        let first = state.lyrics_editors[&layer_id].widget_id(layer_id);

        state.reset_for_project(&project);
        let second = state.lyrics_editors[&layer_id].widget_id(layer_id);

        assert_ne!(first, second);
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
        let context = test_context();
        let text_id = egui::Id::new("undo-owned-by-text-edit");
        let mut text = "入力中".to_owned();
        let _ = crate::headless_paint_output(context.run_ui(Default::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                ui.add(egui::TextEdit::singleline(&mut text).id(text_id))
                    .request_focus();
            });
        }));
        assert!(context.egui_wants_keyboard_input());

        let modifiers = egui::Modifiers::CTRL;
        let output = crate::headless_paint_output(context.run_ui(
            egui::RawInput {
                events: vec![
                    egui::Event::ModifiersChanged(modifiers),
                    egui::Event::Key {
                        key: egui::Key::Z,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    },
                ],
                ..Default::default()
            },
            |context| {
                assert!(!apply_history_shortcut(context, &mut project, &mut state));
                assert!(context.input(|input| !input.events.is_empty()));
                egui::CentralPanel::default().show(context, |ui| {
                    ui.add(egui::TextEdit::singleline(&mut text).id(text_id));
                });
            },
        ));
        drop(output);
        assert_eq!(project, edited);
        assert_eq!(state.history_depths(), (1, 0));
    }
}

/// Keep paint/platform observations from tests that have no renderer.
#[cfg(test)]
pub(crate) fn headless_paint_output(mut output: egui::FullOutput) -> egui::FullOutput {
    egui::FullOutput {
        textures_delta: std::mem::take(&mut output.textures_delta),
        ..Default::default()
    }
    .drop_without_applying_deltas();
    output
}
