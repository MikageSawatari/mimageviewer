//! Opt-in Rhai runner for isolated in-process application tests.
//!
//! The worker evaluates scripts. App/UI access stays behind typed commands;
//! synthetic key input is materialized by `key_input`'s ROOT plugin, while the
//! opt-in native mouse diagnostic calls its OS driver on the worker. App state
//! publication, direct `KeyAction` delivery, and shutdown stay on the UI thread.

#![cfg_attr(all(test, not(feature = "test-script")), allow(dead_code))]

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock, mpsc};
use std::time::{Duration, Instant};

use rhai::{Dynamic, Engine, EvalAltResult, FnPtr, ImmutableString, Map, NativeCallContext};

#[cfg(test)]
use crate::key_input::SyntheticKeyCommandKind;
use crate::key_input::{
    SyntheticInputIssue, SyntheticKeyCommand, SyntheticModifiers, SyntheticNavigationKey,
};
use crate::keymap::{KeyAction, KeyTrigger};

mod capture;
pub(crate) mod pointer_input;

const MAX_SCRIPT_BYTES: u64 = 1024 * 1024;
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(10);
pub(crate) const MAX_ITEM_ROWS_IN_SNAPSHOT: usize = 16;
const EXIT_NOT_SET: i32 = -1;
const EXIT_SCRIPT_FAILURE: i32 = 1;
const EXIT_ENVIRONMENT_FAILURE: i32 = 2;
// App-owned workers get two seconds to join during normal shutdown. Six
// seconds leaves additional scheduling margin while still firing before the
// fixed wgpu device-drop timeout observed on contended Windows GPU systems.
const SHUTDOWN_WATCHDOG_GRACE: Duration = Duration::from_secs(6);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct KeymapLevelObservation {
    pub(crate) frame_nr: u64,
    pub(crate) key: String,
    pub(crate) hold_ids: Vec<u64>,
    pub(crate) held: bool,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) enum TestScriptPaintSourceKind {
    CatalogThumbnail,
    FullOrProcessed,
}

impl TestScriptPaintSourceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::CatalogThumbnail => "catalog_thumbnail",
            Self::FullOrProcessed => "full_or_processed",
        }
    }

    fn preference(self) -> u8 {
        match self {
            Self::CatalogThumbnail => 0,
            Self::FullOrProcessed => 1,
        }
    }
}

/// Identity captured by the producer that selected the texture later painted.
///
/// This is diagnostic evidence only. It must travel with the selected resource;
/// reconstructing it from the current cache would incorrectly relabel a frozen
/// thumbnail after a full-resolution entry arrives.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) struct TestScriptContentProof {
    pub(crate) context_serial: u64,
    pub(crate) items_generation: u64,
    pub(crate) page_index: usize,
    pub(crate) item_identity: String,
    pub(crate) source_texture_id: egui::TextureId,
    pub(crate) source_kind: TestScriptPaintSourceKind,
    /// The selected source is an exact, complete final composite for this page.
    pub(crate) final_composite_complete: bool,
}

/// One image mesh submitted during the latest callback for an exact window.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TestScriptPaintObservation {
    pub(crate) owner: TestScriptWindowIdentity,
    pub(crate) content: TestScriptContentProof,
    pub(crate) texture: egui::TextureId,
    pub(crate) vertices: Vec<([f32; 2], [f32; 2])>,
    pub(crate) clip: [f32; 4],
    pub(crate) viewport_size: [f32; 2],
    pub(crate) pixels_per_point: f32,
    pub(crate) placement: String,
    pub(crate) revision: u64,
}

#[derive(Clone)]
struct PendingPaintObservation {
    content: TestScriptContentProof,
    texture: egui::TextureId,
    vertices: Vec<([f32; 2], [f32; 2])>,
    clip: [f32; 4],
    viewport_size: [f32; 2],
    pixels_per_point: f32,
    placement: String,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum TestScriptSidecarObservation {
    Loaded,
    Imported,
}

thread_local! {
    static DRAWN_PAINT: std::cell::RefCell<std::collections::HashMap<egui::ViewportId, Vec<PendingPaintObservation>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Called immediately after the paint resource submits its mesh. Read the
/// actual egui shape rather than recomputing geometry from a layout DTO.
#[cfg(feature = "test-script")]
pub(crate) fn record_drawn_paint(
    painter: &egui::Painter,
    resource: &crate::gpu_lanczos::FullscreenPaintResource,
    placement: String,
) {
    let Some(content) = resource.test_script_content_proof().cloned() else {
        return;
    };
    let shape = painter.ctx().graphics(|graphics| {
        graphics
            .get(painter.layer_id())
            .and_then(|list| list.all_entries().last().cloned())
    });
    let Some(clipped) = shape else {
        return;
    };
    let egui::Shape::Mesh(mesh) = clipped.shape else {
        return;
    };
    let clip_rect = clipped.clip_rect;
    if mesh.texture_id != resource.paint_texture_id() {
        return;
    }
    let viewport = painter.ctx().viewport_id();
    let size = painter.ctx().viewport_rect().size();
    let pixels_per_point = painter.ctx().pixels_per_point();
    DRAWN_PAINT.with(|drawn| {
        drawn
            .borrow_mut()
            .entry(viewport)
            .or_default()
            .push(PendingPaintObservation {
                content,
                texture: mesh.texture_id,
                vertices: mesh
                    .vertices
                    .iter()
                    .map(|vertex| ([vertex.pos.x, vertex.pos.y], [vertex.uv.x, vertex.uv.y]))
                    .collect(),
                clip: [
                    clip_rect.min.x,
                    clip_rect.min.y,
                    clip_rect.max.x,
                    clip_rect.max.y,
                ],
                viewport_size: [size.x, size.y],
                pixels_per_point,
                placement,
            });
    });
}

fn take_drawn_paint(viewport: egui::ViewportId) -> Vec<PendingPaintObservation> {
    DRAWN_PAINT.with(|drawn| drawn.borrow_mut().remove(&viewport).unwrap_or_default())
}

pub(crate) fn discard_drawn_paint(viewport: egui::ViewportId) {
    let _ = take_drawn_paint(viewport);
}

/// Existing window lifetime owners represented without inventing a shared ID space.
///
/// The root content owner is the registry's main context. Detached host incarnations
/// come from `DetachedWindowManager`; they are not a second test-owned epoch. Both
/// variants also carry the eframe backend token for the exact live `winit::Window`
/// allocation, because an HWND alone can be reused while an older host is still alive.
/// Residence is deliberately absent because Mounted/AtRest is only the storage
/// location of the same logical viewer and host.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) enum TestScriptWindowIdentity {
    Root {
        context_serial: u64,
        hwnd: u64,
        backend_token: u64,
    },
    Detached {
        window_id: u64,
        context_serial: u64,
        viewport_id: egui::ViewportId,
        host_incarnation: u64,
        hwnd: u64,
        backend_token: u64,
    },
}

impl TestScriptWindowIdentity {
    pub(crate) fn role(&self) -> &'static str {
        match self {
            Self::Root { .. } => "root",
            Self::Detached { .. } => "detached",
        }
    }

    pub(crate) fn window_id(&self) -> Option<u64> {
        match self {
            Self::Root { .. } => None,
            Self::Detached { window_id, .. } => Some(*window_id),
        }
    }

    pub(crate) fn context_serial(&self) -> u64 {
        match self {
            Self::Root { context_serial, .. } | Self::Detached { context_serial, .. } => {
                *context_serial
            }
        }
    }

    pub(crate) fn viewport_id(&self) -> egui::ViewportId {
        match self {
            Self::Root { .. } => egui::ViewportId::ROOT,
            Self::Detached { viewport_id, .. } => *viewport_id,
        }
    }

    pub(crate) fn host_incarnation(&self) -> Option<u64> {
        match self {
            Self::Root { .. } => None,
            Self::Detached {
                host_incarnation, ..
            } => Some(*host_incarnation),
        }
    }

    pub(crate) fn hwnd(&self) -> u64 {
        match self {
            Self::Root { hwnd, .. } | Self::Detached { hwnd, .. } => *hwnd,
        }
    }

    pub(crate) fn backend_token(&self) -> u64 {
        match self {
            Self::Root { backend_token, .. } | Self::Detached { backend_token, .. } => {
                *backend_token
            }
        }
    }

    pub(crate) fn matches_backend_witness(
        &self,
        witness: eframe::miv_test_script_window_witness::WindowWitness,
    ) -> bool {
        self.viewport_id() == witness.viewport_id()
            && self.hwnd() == witness.hwnd()
            && self.backend_token() == witness.token()
    }

    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Root {
                context_serial,
                hwnd,
                backend_token,
            } => format!("root/context={context_serial}/hwnd=0x{hwnd:x}/backend={backend_token}"),
            Self::Detached {
                window_id,
                context_serial,
                viewport_id,
                host_incarnation,
                hwnd,
                backend_token,
            } => format!(
                "detached/window={window_id}/context={context_serial}/viewport={viewport_id:?}/host={host_incarnation}/hwnd=0x{hwnd:x}/backend={backend_token}"
            ),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum TestScriptActionSelection {
    #[default]
    LegacyImplicit,
    Targeted(TestScriptWindowIdentity),
}

impl TestScriptActionSelection {
    fn mode(&self) -> &'static str {
        match self {
            Self::LegacyImplicit => "legacy_implicit",
            Self::Targeted(_) => "targeted",
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct TestScriptPaintEvidenceKey {
    owner: TestScriptWindowIdentity,
    content: TestScriptContentProof,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TestScriptSeekStripSnapshot {
    pub(crate) state: String,
    pub(crate) session_id: Option<u64>,
    pub(crate) source_epoch: Option<u64>,
    pub(crate) items_generation: Option<u64>,
    pub(crate) owner_fs_idx: Option<usize>,
    pub(crate) layout_revision: Option<u64>,
    pub(crate) mode: String,
    pub(crate) span: String,
    pub(crate) visible_count: Option<usize>,
    pub(crate) axis_cell_count: Option<usize>,
    pub(crate) last_sent_request_id: Option<u64>,
    pub(crate) last_finished_request_id: Option<u64>,
    pub(crate) worker_instance_id: Option<u64>,
    pub(crate) decoder_open_count: Option<u64>,
    pub(crate) worker_status: String,
    pub(crate) receipt_present: bool,
    pub(crate) receipt_visible: bool,
    pub(crate) receipt_source_epoch: Option<u64>,
    pub(crate) receipt_session_id: Option<u64>,
    pub(crate) receipt_generation: Option<u64>,
    pub(crate) receipt_layout_revision: Option<u64>,
    pub(crate) receipt_reported_count: Option<usize>,
    pub(crate) receipt_window_applied: bool,
}

impl TestScriptSeekStripSnapshot {
    pub(crate) fn closed() -> Self {
        Self {
            state: "closed".to_string(),
            session_id: None,
            source_epoch: None,
            items_generation: None,
            owner_fs_idx: None,
            layout_revision: None,
            mode: "none".to_string(),
            span: "none".to_string(),
            visible_count: None,
            axis_cell_count: None,
            last_sent_request_id: None,
            last_finished_request_id: None,
            worker_instance_id: None,
            decoder_open_count: None,
            worker_status: "none".to_string(),
            receipt_present: false,
            receipt_visible: false,
            receipt_source_epoch: None,
            receipt_session_id: None,
            receipt_generation: None,
            receipt_layout_revision: None,
            receipt_reported_count: None,
            receipt_window_applied: false,
        }
    }

    fn to_rhai_map(&self) -> Map {
        let mut map = Map::new();
        map.insert("state".into(), self.state.clone().into());
        map.insert("session_id".into(), optional_rhai_u64(self.session_id));
        map.insert("source_epoch".into(), optional_rhai_u64(self.source_epoch));
        map.insert(
            "items_generation".into(),
            optional_rhai_u64(self.items_generation),
        );
        map.insert(
            "owner_fs_idx".into(),
            optional_rhai_usize(self.owner_fs_idx),
        );
        map.insert(
            "layout_revision".into(),
            optional_rhai_u64(self.layout_revision),
        );
        map.insert("mode".into(), self.mode.clone().into());
        map.insert("span".into(), self.span.clone().into());
        map.insert(
            "visible_count".into(),
            optional_rhai_usize(self.visible_count),
        );
        map.insert(
            "axis_cell_count".into(),
            optional_rhai_usize(self.axis_cell_count),
        );
        map.insert(
            "last_sent_request_id".into(),
            optional_rhai_u64(self.last_sent_request_id),
        );
        map.insert(
            "last_finished_request_id".into(),
            optional_rhai_u64(self.last_finished_request_id),
        );
        map.insert(
            "worker_instance_id".into(),
            optional_rhai_u64(self.worker_instance_id),
        );
        map.insert(
            "decoder_open_count".into(),
            optional_rhai_u64(self.decoder_open_count),
        );
        map.insert("worker_status".into(), self.worker_status.clone().into());
        map.insert("receipt_present".into(), self.receipt_present.into());
        map.insert("receipt_visible".into(), self.receipt_visible.into());
        map.insert(
            "receipt_source_epoch".into(),
            optional_rhai_u64(self.receipt_source_epoch),
        );
        map.insert(
            "receipt_session_id".into(),
            optional_rhai_u64(self.receipt_session_id),
        );
        map.insert(
            "receipt_generation".into(),
            optional_rhai_u64(self.receipt_generation),
        );
        map.insert(
            "receipt_layout_revision".into(),
            optional_rhai_u64(self.receipt_layout_revision),
        );
        map.insert(
            "receipt_reported_count".into(),
            optional_rhai_usize(self.receipt_reported_count),
        );
        map.insert(
            "receipt_window_applied".into(),
            self.receipt_window_applied.into(),
        );
        map
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TestScriptWindowPresentation {
    Root,
    ActiveImmediate,
    ParkedLiveImmediate,
    PassiveDeferredFrozen,
    Other,
}

impl TestScriptWindowPresentation {
    fn as_str(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::ActiveImmediate => "active_immediate",
            Self::ParkedLiveImmediate => "parked_live_immediate",
            Self::PassiveDeferredFrozen => "passive_deferred_frozen",
            Self::Other => "other",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TestScriptWindowSnapshot {
    pub(crate) identity: Option<TestScriptWindowIdentity>,
    pub(crate) role: String,
    pub(crate) window_id: Option<u64>,
    pub(crate) context_serial: u64,
    pub(crate) viewport_id: egui::ViewportId,
    pub(crate) host_incarnation: Option<u64>,
    pub(crate) hwnd: Option<u64>,
    pub(crate) backend_token: Option<u64>,
    pub(crate) residence: String,
    pub(crate) presentation: TestScriptWindowPresentation,
    pub(crate) media_kind: String,
    pub(crate) page_index: Option<usize>,
    pub(crate) items_generation: u64,
    pub(crate) item_identity: String,
    pub(crate) selected_item_identity: String,
    pub(crate) page_ready: bool,
    pub(crate) viewport_rendered: bool,
    pub(crate) viewport_revision: u64,
    pub(crate) paint_matches_current_page: bool,
    pub(crate) full_texture_painted: bool,
    pub(crate) paint_source: String,
    pub(crate) paint_source_texture: String,
    pub(crate) painted_page_index: Option<usize>,
    pub(crate) paint_revision: u64,
    pub(crate) paints: Vec<TestScriptPaintObservation>,
    pub(crate) sidecar_imported: bool,
    pub(crate) sidecar_loaded: bool,
    pub(crate) seek_strip: TestScriptSeekStripSnapshot,
}

impl TestScriptWindowSnapshot {
    fn current_content_matches(&self, proof: &TestScriptContentProof) -> bool {
        self.context_serial == proof.context_serial
            && self.items_generation == proof.items_generation
            && self.page_index == Some(proof.page_index)
            && self.item_identity == proof.item_identity
    }

    fn accepts_owner(&self, owner: &TestScriptWindowIdentity) -> bool {
        self.identity.as_ref() == Some(owner)
    }

    fn to_rhai_map(&self) -> Map {
        let mut map = Map::new();
        map.insert("role".into(), self.role.clone().into());
        map.insert(
            "window_id".into(),
            self.window_id
                .map(|value| Dynamic::from(saturating_rhai_int(value)))
                .unwrap_or(Dynamic::UNIT),
        );
        map.insert(
            "context_serial".into(),
            saturating_rhai_int(self.context_serial).into(),
        );
        map.insert("viewport".into(), format!("{:?}", self.viewport_id).into());
        map.insert(
            "host_incarnation".into(),
            self.host_incarnation
                .map(|value| Dynamic::from(saturating_rhai_int(value)))
                .unwrap_or(Dynamic::UNIT),
        );
        map.insert(
            "hwnd".into(),
            self.hwnd
                .map(|value| Dynamic::from(format!("0x{value:x}")))
                .unwrap_or(Dynamic::UNIT),
        );
        map.insert(
            "backend_token".into(),
            self.backend_token
                .map(|value| Dynamic::from(saturating_rhai_int(value)))
                .unwrap_or(Dynamic::UNIT),
        );
        map.insert("host_ready".into(), self.identity.is_some().into());
        map.insert("residence".into(), self.residence.clone().into());
        map.insert("presentation".into(), self.presentation.as_str().into());
        map.insert("media_kind".into(), self.media_kind.clone().into());
        map.insert(
            "page_index".into(),
            self.page_index
                .map_or(-1, |value| i64::try_from(value).unwrap_or(i64::MAX))
                .into(),
        );
        map.insert(
            "items_generation".into(),
            saturating_rhai_int(self.items_generation).into(),
        );
        map.insert("item_identity".into(), self.item_identity.clone().into());
        map.insert(
            "selected_item_identity".into(),
            self.selected_item_identity.clone().into(),
        );
        map.insert("page_ready".into(), self.page_ready.into());
        map.insert("viewport_rendered".into(), self.viewport_rendered.into());
        map.insert(
            "viewport_revision".into(),
            saturating_rhai_int(self.viewport_revision).into(),
        );
        map.insert(
            "paint_matches_current_page".into(),
            self.paint_matches_current_page.into(),
        );
        map.insert(
            "full_texture_painted".into(),
            self.full_texture_painted.into(),
        );
        map.insert("paint_source".into(), self.paint_source.clone().into());
        map.insert(
            "paint_source_texture".into(),
            self.paint_source_texture.clone().into(),
        );
        map.insert(
            "painted_page_index".into(),
            self.painted_page_index
                .map_or(-1, |value| i64::try_from(value).unwrap_or(i64::MAX))
                .into(),
        );
        map.insert(
            "paint_revision".into(),
            saturating_rhai_int(self.paint_revision).into(),
        );
        map.insert(
            "paints".into(),
            self.paints
                .iter()
                .map(|paint| {
                    let mut item = Map::new();
                    item.insert(
                        "window_id".into(),
                        optional_rhai_u64(paint.owner.window_id()),
                    );
                    item.insert(
                        "context_serial".into(),
                        saturating_rhai_int(paint.owner.context_serial()).into(),
                    );
                    item.insert(
                        "item_identity".into(),
                        paint.content.item_identity.clone().into(),
                    );
                    item.insert(
                        "page_index".into(),
                        saturating_rhai_int(paint.content.page_index as u64).into(),
                    );
                    item.insert(
                        "items_generation".into(),
                        saturating_rhai_int(paint.content.items_generation).into(),
                    );
                    item.insert("source".into(), paint.content.source_kind.as_str().into());
                    item.insert(
                        "final_composite_complete".into(),
                        paint.content.final_composite_complete.into(),
                    );
                    item.insert(
                        "source_texture".into(),
                        format!("{:?}", paint.content.source_texture_id).into(),
                    );
                    item.insert("texture".into(), format!("{:?}", paint.texture).into());
                    item.insert(
                        "vertices".into(),
                        Dynamic::from_array(
                            paint
                                .vertices
                                .iter()
                                .map(|(pos, uv)| {
                                    Dynamic::from_array(
                                        [
                                            pos[0] as rhai::FLOAT,
                                            pos[1] as rhai::FLOAT,
                                            uv[0] as rhai::FLOAT,
                                            uv[1] as rhai::FLOAT,
                                        ]
                                        .into_iter()
                                        .map(Dynamic::from)
                                        .collect(),
                                    )
                                })
                                .collect(),
                        ),
                    );
                    item.insert(
                        "clip".into(),
                        Dynamic::from_array(
                            paint
                                .clip
                                .into_iter()
                                .map(|x| Dynamic::from(x as rhai::FLOAT))
                                .collect(),
                        ),
                    );
                    item.insert(
                        "viewport_size".into(),
                        Dynamic::from_array(
                            paint
                                .viewport_size
                                .into_iter()
                                .map(|x| Dynamic::from(x as rhai::FLOAT))
                                .collect(),
                        ),
                    );
                    item.insert(
                        "pixels_per_point".into(),
                        (paint.pixels_per_point as rhai::FLOAT).into(),
                    );
                    item.insert("placement".into(), paint.placement.clone().into());
                    item.insert(
                        "revision".into(),
                        saturating_rhai_int(paint.revision).into(),
                    );
                    Dynamic::from_map(item)
                })
                .collect::<rhai::Array>()
                .into(),
        );
        map.insert("sidecar_imported".into(), self.sidecar_imported.into());
        map.insert("sidecar_loaded".into(), self.sidecar_loaded.into());
        map.insert(
            "seek_strip".into(),
            Dynamic::from_map(self.seek_strip.to_rhai_map()),
        );
        map
    }
}

fn saturating_rhai_int(value: u64) -> rhai::INT {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn optional_rhai_u64(value: Option<u64>) -> Dynamic {
    value
        .map(|value| Dynamic::from(saturating_rhai_int(value)))
        .unwrap_or_else(|| Dynamic::from(-1_i64))
}

fn optional_rhai_usize(value: Option<usize>) -> Dynamic {
    value
        .map(|value| Dynamic::from(i64::try_from(value).unwrap_or(i64::MAX)))
        .unwrap_or_else(|| Dynamic::from(-1_i64))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TestScriptSortPopupRow {
    pub(crate) label: String,
    pub(crate) disabled: bool,
    pub(crate) visible: bool,
    pub(crate) visible_x: bool,
    pub(crate) visible_y: bool,
    pub(crate) geometry: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TestScriptCollectionSortPopup {
    pub(crate) open: bool,
    pub(crate) needs_scrolling: bool,
    pub(crate) within_screen: bool,
    pub(crate) sort_control_locked: bool,
    pub(crate) popup_ui_enabled: bool,
    pub(crate) viewport: String,
    pub(crate) rows: Vec<TestScriptSortPopupRow>,
    pub(crate) rendered_tooltip: Option<String>,
}

impl TestScriptCollectionSortPopup {
    fn to_rhai_map(&self) -> Map {
        let mut map = Map::new();
        map.insert("open".into(), self.open.into());
        map.insert("needs_scrolling".into(), self.needs_scrolling.into());
        map.insert("within_screen".into(), self.within_screen.into());
        map.insert(
            "sort_control_locked".into(),
            self.sort_control_locked.into(),
        );
        map.insert("popup_ui_enabled".into(), self.popup_ui_enabled.into());
        map.insert("viewport".into(), self.viewport.clone().into());
        map.insert(
            "tooltip_open".into(),
            self.rendered_tooltip.is_some().into(),
        );
        map.insert(
            "tooltip_text".into(),
            Dynamic::from(self.rendered_tooltip.clone().unwrap_or_default()),
        );
        map.insert(
            "rows".into(),
            self.rows
                .iter()
                .map(|row| {
                    let mut value = Map::new();
                    value.insert("label".into(), Dynamic::from(row.label.clone()));
                    value.insert("disabled".into(), row.disabled.into());
                    value.insert("visible".into(), row.visible.into());
                    value.insert("visible_x".into(), row.visible_x.into());
                    value.insert("visible_y".into(), row.visible_y.into());
                    value.insert("geometry".into(), row.geometry.clone().into());
                    Dynamic::from_map(value)
                })
                .collect::<rhai::Array>()
                .into(),
        );
        map
    }
}

#[derive(Default)]
struct CollectionSortPopupFrame {
    frame_nr: u64,
    snapshot: TestScriptCollectionSortPopup,
}

thread_local! {
    static COLLECTION_SORT_POPUP_FRAME: RefCell<CollectionSortPopupFrame> =
        RefCell::new(CollectionSortPopupFrame::default());
}

pub(crate) fn begin_collection_sort_popup(
    ctx: &egui::Context,
    sort_control_locked: bool,
    popup_ui_enabled: bool,
) {
    if ctx.viewport_id() != egui::ViewportId::ROOT {
        return;
    }
    COLLECTION_SORT_POPUP_FRAME.with(|observed| {
        let mut observed = observed.borrow_mut();
        observed.frame_nr = ctx.cumulative_frame_nr();
        observed.snapshot = TestScriptCollectionSortPopup {
            open: true,
            within_screen: true,
            sort_control_locked,
            popup_ui_enabled,
            viewport: format!(
                "{:?} ppp={:.2}",
                ctx.viewport_rect(),
                ctx.pixels_per_point()
            ),
            ..Default::default()
        };
    });
}

pub(crate) fn record_collection_sort_popup_row(
    label: &str,
    response: &egui::Response,
    clip: egui::Rect,
) {
    let ctx = &response.ctx;
    if ctx.viewport_id() != egui::ViewportId::ROOT {
        return;
    }
    let row = response.rect;
    let visible_x = row.left() >= clip.left() - 1.0 && row.right() <= clip.right() + 1.0;
    let visible_y = row.top() >= clip.top() - 1.0 && row.bottom() <= clip.bottom() + 1.0;
    let visible = visible_x && visible_y;
    let screen = ctx.viewport_rect();
    let within_screen = row.left() >= screen.left() - 1.0
        && row.right() <= screen.right() + 1.0
        && row.top() >= screen.top() - 1.0
        && row.bottom() <= screen.bottom() + 1.0;
    COLLECTION_SORT_POPUP_FRAME.with(|observed| {
        let mut observed = observed.borrow_mut();
        if observed.frame_nr != ctx.cumulative_frame_nr() || observed.snapshot.rows.len() >= 16 {
            return;
        }
        observed.snapshot.needs_scrolling |= !visible_y;
        observed.snapshot.within_screen &= within_screen;
        observed.snapshot.rows.push(TestScriptSortPopupRow {
            label: label.to_owned(),
            disabled: !response.enabled(),
            visible,
            visible_x,
            visible_y,
            geometry: format!("row={row:?} clip={clip:?} viewport={screen:?}"),
        });
    });
    register_sort_popup_pointer_row(label, response, clip);
}

/// Called only from the real disabled tooltip's content closure, after its
/// text label was laid out inside the open popup.
pub(crate) fn record_collection_sort_tooltip_rendered(
    ui: &egui::Ui,
    label: &egui::Response,
    text: &str,
) {
    let ctx = ui.ctx();
    if ctx.viewport_id() != egui::ViewportId::ROOT
        || !ui.is_rect_visible(label.rect)
        || !label.rect.intersects(ctx.viewport_rect())
    {
        return;
    }
    COLLECTION_SORT_POPUP_FRAME.with(|observed| {
        let mut observed = observed.borrow_mut();
        if observed.snapshot.open && observed.frame_nr == ctx.cumulative_frame_nr() {
            observed.snapshot.rendered_tooltip = Some(text.to_owned());
        }
    });
}

pub(crate) fn collection_sort_popup_snapshot(ctx: &egui::Context) -> TestScriptCollectionSortPopup {
    COLLECTION_SORT_POPUP_FRAME.with(|observed| {
        let observed = observed.borrow();
        if observed.snapshot.open
            && observed.frame_nr.saturating_add(1) >= ctx.cumulative_frame_nr()
        {
            observed.snapshot.clone()
        } else {
            TestScriptCollectionSortPopup::default()
        }
    })
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TestScriptSnapshot {
    pub(crate) is_fullscreen: bool,
    pub(crate) always_on_top: bool,
    pub(crate) window_visible: bool,
    pub(crate) fs_idx: i64,
    pub(crate) items_generation: i64,
    pub(crate) folder_load_requests: i64,
    pub(crate) focused: bool,
    pub(crate) target_viewport: String,
    pub(crate) target_registered: bool,
    pub(crate) target_rendered: bool,
    /// 現在のフォルダに並んでいる item 数。
    ///
    /// `pending_thumbs == 0` だけでは「全部終わった」と「まだ何も始まっていない」を
    /// 区別できない。落ち着いたことを待つ条件には `items_len > 0` を併せて使う。
    pub(crate) items_len: i64,
    pub(crate) item_names: Vec<String>,
    pub(crate) item_ratings: Vec<i64>,
    pub(crate) sort_order: String,
    pub(crate) collection_runtime_phase: String,
    pub(crate) collection_runtime_error: String,
    pub(crate) collection_seeded_id_present: bool,
    pub(crate) grid_surface: String,
    pub(crate) current_folder_path: String,
    pub(crate) startup_open_pending: bool,
    pub(crate) collection_root_visible: bool,
    pub(crate) collection_id: String,
    pub(crate) collection_order_mode: String,
    pub(crate) collection_standard_sort: String,
    pub(crate) collection_revision: i64,
    pub(crate) collection_sort_popup: TestScriptCollectionSortPopup,
    pub(crate) rating_sort_unrated_position: String,
    pub(crate) preferences_open: bool,
    pub(crate) preferences_page: String,
    pub(crate) preferences_draft_unrated_position: String,
    pub(crate) smart_folder_busy: bool,
    pub(crate) smart_folder_name: String,
    pub(crate) smart_folder_root_visible: bool,
    pub(crate) smart_folder_session_phase: String,
    pub(crate) pending_thumbs: i64,
    pub(crate) spread_mode: String,
    pub(crate) continuous_reading: bool,
    pub(crate) current_is_still_image: bool,
    pub(crate) music_view_active: bool,
    pub(crate) modal_open: bool,
    pub(crate) context_menu_open: bool,
    pub(crate) popup_open: bool,
    pub(crate) ime_active: bool,
    pub(crate) text_input_or_pending_focus: bool,
    pub(crate) overlay_edit_active: bool,
    pub(crate) capture_region_selection: bool,
    pub(crate) fullscreen_raw_key_permit: bool,
    pub(crate) has_previous_page: bool,
    pub(crate) has_next_page: bool,
    /// Consecutive frames whose page-turn decision deferred texture uploads. A number that keeps
    /// climbing while nothing is being pressed is the livelock, not slow loading.
    /// Which continuation the viewer is in: `paged`, `vertical` or `horizontal`. `continuous_reading`
    /// says only that it is not paged, and the two continuations lay pages out differently enough
    /// that a scenario reproducing a layout-dependent failure has to name the one it means.
    pub(crate) reading_flow: String,
    pub(crate) upload_deferral_streak: i64,
    /// Why the page has no stand-in to show, or empty. See `PassthroughUnavailable`.
    pub(crate) passthrough_unavailable: String,
    pub(crate) keymap_level_observations: Vec<KeymapLevelObservation>,
    pub(crate) windows: Vec<TestScriptWindowSnapshot>,
    pub(crate) host_styles: Vec<TestScriptHostStyle>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TestScriptHostStyle {
    pub(crate) role: String,
    pub(crate) window_id: Option<u64>,
    pub(crate) presentation: TestScriptWindowPresentation,
    pub(crate) viewport: String,
    pub(crate) viewport_id: Option<egui::ViewportId>,
    pub(crate) hwnd: u64,
    pub(crate) backend_token: Option<u64>,
    pub(crate) topmost: bool,
    pub(crate) noactivate: bool,
    pub(crate) minimized: bool,
    pub(crate) visible: bool,
    pub(crate) owner_hwnd: u64,
    pub(crate) foreground_hwnd: u64,
}

impl TestScriptHostStyle {
    fn to_rhai_map(&self) -> Map {
        let mut map = Map::new();
        map.insert("role".into(), self.role.clone().into());
        map.insert(
            "window_id".into(),
            self.window_id
                .map(|id| Dynamic::from(saturating_rhai_int(id)))
                .unwrap_or(Dynamic::UNIT),
        );
        map.insert("presentation".into(), self.presentation.as_str().into());
        map.insert("viewport".into(), self.viewport.clone().into());
        map.insert("hwnd".into(), format!("0x{:x}", self.hwnd).into());
        map.insert(
            "backend_token".into(),
            self.backend_token
                .map(|token| Dynamic::from(saturating_rhai_int(token)))
                .unwrap_or(Dynamic::UNIT),
        );
        map.insert("topmost".into(), self.topmost.into());
        map.insert("noactivate".into(), self.noactivate.into());
        map.insert("minimized".into(), self.minimized.into());
        map.insert("visible".into(), self.visible.into());
        map.insert(
            "owner_hwnd".into(),
            format!("0x{:x}", self.owner_hwnd).into(),
        );
        map.insert(
            "foreground_hwnd".into(),
            format!("0x{:x}", self.foreground_hwnd).into(),
        );
        map
    }
}

impl Default for TestScriptSnapshot {
    fn default() -> Self {
        Self {
            is_fullscreen: false,
            always_on_top: false,
            window_visible: true,
            fs_idx: -1,
            items_generation: 0,
            folder_load_requests: 0,
            focused: false,
            target_viewport: "unregistered".to_string(),
            target_registered: false,
            target_rendered: false,
            items_len: 0,
            item_names: Vec::new(),
            item_ratings: Vec::new(),
            sort_order: String::new(),
            collection_runtime_phase: String::new(),
            collection_runtime_error: String::new(),
            collection_seeded_id_present: false,
            grid_surface: String::new(),
            current_folder_path: String::new(),
            startup_open_pending: false,
            collection_root_visible: false,
            collection_id: String::new(),
            collection_order_mode: String::new(),
            collection_standard_sort: String::new(),
            collection_revision: -1,
            collection_sort_popup: TestScriptCollectionSortPopup::default(),
            rating_sort_unrated_position: String::new(),
            preferences_open: false,
            preferences_page: String::new(),
            preferences_draft_unrated_position: String::new(),
            smart_folder_busy: false,
            smart_folder_name: String::new(),
            smart_folder_root_visible: false,
            smart_folder_session_phase: String::new(),
            pending_thumbs: 0,
            spread_mode: "Single".to_string(),
            continuous_reading: false,
            current_is_still_image: false,
            music_view_active: false,
            modal_open: false,
            context_menu_open: false,
            popup_open: false,
            ime_active: false,
            text_input_or_pending_focus: false,
            overlay_edit_active: false,
            capture_region_selection: false,
            fullscreen_raw_key_permit: false,
            has_previous_page: false,
            has_next_page: false,
            reading_flow: "paged".to_string(),
            upload_deferral_streak: 0,
            passthrough_unavailable: String::new(),
            keymap_level_observations: Vec::new(),
            windows: Vec::new(),
            host_styles: Vec::new(),
        }
    }
}

impl TestScriptSnapshot {
    fn to_rhai_map(&self) -> Map {
        let mut map = Map::new();
        macro_rules! insert {
            ($field:ident) => {
                map.insert(
                    stringify!($field).into(),
                    Dynamic::from(self.$field.clone()),
                );
            };
        }
        insert!(is_fullscreen);
        insert!(always_on_top);
        insert!(window_visible);
        insert!(fs_idx);
        insert!(items_generation);
        insert!(folder_load_requests);
        insert!(focused);
        insert!(target_viewport);
        insert!(target_registered);
        insert!(target_rendered);
        insert!(items_len);
        map.insert(
            "item_names".into(),
            self.item_names
                .iter()
                .cloned()
                .map(Dynamic::from)
                .collect::<rhai::Array>()
                .into(),
        );
        map.insert(
            "item_ratings".into(),
            self.item_ratings
                .iter()
                .copied()
                .map(Dynamic::from)
                .collect::<rhai::Array>()
                .into(),
        );
        insert!(sort_order);
        insert!(collection_runtime_phase);
        insert!(collection_runtime_error);
        insert!(collection_seeded_id_present);
        insert!(grid_surface);
        insert!(current_folder_path);
        insert!(startup_open_pending);
        insert!(collection_root_visible);
        insert!(collection_id);
        insert!(collection_order_mode);
        insert!(collection_standard_sort);
        insert!(collection_revision);
        map.insert(
            "collection_sort_popup".into(),
            Dynamic::from_map(self.collection_sort_popup.to_rhai_map()),
        );
        insert!(rating_sort_unrated_position);
        insert!(preferences_open);
        insert!(preferences_page);
        insert!(preferences_draft_unrated_position);
        insert!(smart_folder_busy);
        insert!(smart_folder_name);
        insert!(smart_folder_root_visible);
        insert!(smart_folder_session_phase);
        insert!(pending_thumbs);
        insert!(spread_mode);
        insert!(continuous_reading);
        insert!(current_is_still_image);
        insert!(music_view_active);
        insert!(modal_open);
        insert!(context_menu_open);
        insert!(popup_open);
        insert!(ime_active);
        insert!(text_input_or_pending_focus);
        insert!(overlay_edit_active);
        insert!(capture_region_selection);
        insert!(fullscreen_raw_key_permit);
        insert!(has_previous_page);
        insert!(has_next_page);
        insert!(reading_flow);
        insert!(upload_deferral_streak);
        map.insert(
            "host_styles".into(),
            self.host_styles
                .iter()
                .map(|style| Dynamic::from_map(style.to_rhai_map()))
                .collect::<rhai::Array>()
                .into(),
        );
        insert!(passthrough_unavailable);
        map.insert(
            "windows".into(),
            self.windows
                .iter()
                .map(|window| Dynamic::from_map(window.to_rhai_map()))
                .collect::<rhai::Array>()
                .into(),
        );
        map
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScriptOutcomeKind {
    Success,
    ScriptFailure,
    EnvironmentFailure,
}

impl ScriptOutcomeKind {
    fn exit_code(self) -> i32 {
        match self {
            Self::Success => 0,
            Self::ScriptFailure => EXIT_SCRIPT_FAILURE,
            Self::EnvironmentFailure => EXIT_ENVIRONMENT_FAILURE,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::ScriptFailure => "script_failure",
            Self::EnvironmentFailure => "environment_failure",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ScriptOutcome {
    kind: ScriptOutcomeKind,
    message: String,
}

impl ScriptOutcome {
    fn success() -> Self {
        Self {
            kind: ScriptOutcomeKind::Success,
            message: "script completed".to_string(),
        }
    }

    fn script_failure(message: impl Into<String>) -> Self {
        Self {
            kind: ScriptOutcomeKind::ScriptFailure,
            message: message.into(),
        }
    }

    fn environment_failure(message: impl Into<String>) -> Self {
        Self {
            kind: ScriptOutcomeKind::EnvironmentFailure,
            message: message.into(),
        }
    }

    fn override_with_environment_failure(&mut self, message: impl Into<String>) {
        self.kind = ScriptOutcomeKind::EnvironmentFailure;
        self.message = message.into();
    }
}

#[derive(Debug)]
struct PreconditionTrace {
    name: &'static str,
    satisfied: bool,
    timeout_ms: Option<u64>,
    elapsed_ms: u64,
    target_registered: Option<bool>,
    focused: Option<bool>,
}

#[derive(Debug)]
enum UiCommand {
    Key(SyntheticKeyCommand),
    Cancel(Instant),
    SetRepeat {
        delay: Duration,
        hz: f64,
    },
    RunAction {
        action: KeyAction,
        selection: TestScriptActionSelection,
        applied: mpsc::SyncSender<Result<(), String>>,
    },
    SmokeAction(UiSmokeAction),
    ClickWidget {
        label: String,
        reply: mpsc::SyncSender<Result<(), String>>,
    },
    SortPopupPointer {
        label: String,
        kind: SortPopupPointerKind,
        reply: mpsc::SyncSender<Result<(), String>>,
    },
    DetailsPreviewPointer {
        reply: mpsc::SyncSender<Result<(egui::Pos2, f32), String>>,
    },
    ValidateSelectedOwner {
        expected_identity: TestScriptWindowIdentity,
        reply: mpsc::SyncSender<Result<(), String>>,
    },
    Capture {
        label: String,
        scope: CaptureScope,
        selection: TestScriptActionSelection,
        reply: mpsc::SyncSender<Result<(), String>>,
    },
    Log(String),
    Precondition(PreconditionTrace),
    Finished(ScriptOutcome),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CaptureScope {
    Selected,
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UiSmokeAction {
    OpenThumbnailPreferences,
    OpenFirstSmartFolder,
    OpenSeededCollection,
    AlwaysOnTopOn,
    AlwaysOnTopOff,
    MinimizeRoot,
    RestoreRoot,
    HideToTray,
    RestoreFromTray,
    CloseFullscreen,
    ToggleDetachedMode,
    EnableIndependentWindows,
}

pub(crate) const SEEDED_COLLECTION_SMOKE_ID: &str = "80f58851-997b-4b80-90bc-f50bb1d2523e";

pub(crate) fn seeded_collection_smoke_id() -> crate::collection_store::CollectionId {
    crate::collection_store::CollectionId::from_uuid(
        uuid::Uuid::parse_str(SEEDED_COLLECTION_SMOKE_ID).expect("fixed smoke Collection ID"),
    )
}

#[derive(Clone, Copy)]
enum WidgetClickPhase {
    Down,
    Up,
    Hover,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SortPopupPointerKind {
    Hover,
    Click,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WidgetPointerRequestKind {
    ClickEnabled,
    SortPopup(SortPopupPointerKind),
}

struct WidgetPointerRequest {
    label: String,
    kind: WidgetPointerRequestKind,
    reply: mpsc::SyncSender<Result<(), String>>,
}

struct WidgetClick {
    point: egui::Pos2,
    phase: WidgetClickPhase,
}

#[derive(Default)]
struct WidgetClickDriver {
    requested: Option<WidgetPointerRequest>,
    active: Option<WidgetClick>,
    details_preview_move: Option<mpsc::SyncSender<Result<(egui::Pos2, f32), String>>>,
}

thread_local! {
    static WIDGET_CLICK_DRIVER: RefCell<WidgetClickDriver> =
        RefCell::new(WidgetClickDriver::default());
}

fn request_widget_click(
    label: String,
    reply: mpsc::SyncSender<Result<(), String>>,
) -> Result<(), String> {
    request_widget_pointer(label, WidgetPointerRequestKind::ClickEnabled, reply)
}

fn request_widget_pointer(
    label: String,
    kind: WidgetPointerRequestKind,
    reply: mpsc::SyncSender<Result<(), String>>,
) -> Result<(), String> {
    WIDGET_CLICK_DRIVER.with(|driver| {
        let mut driver = driver.borrow_mut();
        if driver.requested.is_some() || driver.active.is_some() {
            return Err("another widget click is still in progress".to_string());
        }
        driver.requested = Some(WidgetPointerRequest { label, kind, reply });
        Ok(())
    })
}

/// Register the actual interactive rectangle; a pending click scrolls its widget into view.
/// The button/radio handler still sees an ordinary egui pointer press and release.
pub(crate) fn register_clickable_widget(label: &str, response: &egui::Response) {
    WIDGET_CLICK_DRIVER.with(|driver| {
        let mut driver = driver.borrow_mut();
        if !driver.requested.as_ref().is_some_and(|request| {
            request.label == label && request.kind == WidgetPointerRequestKind::ClickEnabled
        }) {
            return;
        }
        if !response.interact_rect.contains(response.rect.center()) {
            response.scroll_to_me(Some(egui::Align::Center));
            return;
        }
        if !response.enabled() {
            return;
        }
        let point = response.rect.center();
        let request = driver.requested.take().expect("matching request exists");
        if request.reply.send(Ok(())).is_ok() {
            driver.active = Some(WidgetClick {
                point,
                phase: WidgetClickPhase::Down,
            });
        }
    });
}

/// Sort rows may be disabled. This diagnostic pointer path still sends ordinary
/// egui pointer events, leaving the production response to reject the click.
fn register_sort_popup_pointer_row(label: &str, response: &egui::Response, clip: egui::Rect) {
    WIDGET_CLICK_DRIVER.with(|driver| {
        let mut driver = driver.borrow_mut();
        let Some(request) = driver.requested.as_ref() else {
            return;
        };
        if request.label != label {
            return;
        }
        let WidgetPointerRequestKind::SortPopup(kind) = request.kind else {
            return;
        };
        let point = response.rect.center();
        if !clip.contains(point) {
            return;
        }
        let request = driver.requested.take().expect("matching request exists");
        if request.reply.send(Ok(())).is_ok() {
            driver.active = Some(WidgetClick {
                point,
                phase: match kind {
                    SortPopupPointerKind::Hover => WidgetClickPhase::Hover,
                    SortPopupPointerKind::Click => WidgetClickPhase::Down,
                },
            });
        }
    });
}

/// Publish the actual visible preview cell, so the worker can move the OS pointer to it.
/// No synthetic egui event is injected for this path.
pub(crate) fn register_details_preview_pointer(response: &egui::Response, clip: egui::Rect) {
    WIDGET_CLICK_DRIVER.with(|driver| {
        let mut driver = driver.borrow_mut();
        let Some(reply) = driver.details_preview_move.as_ref() else {
            return;
        };
        let point = response.rect.center();
        if !response.interact_rect.contains(point) || !clip.contains(point) {
            return;
        }
        let _ = reply.send(Ok((point, response.ctx.pixels_per_point())));
        driver.details_preview_move = None;
    });
}

/// Called by the synthetic-input plugin before egui processes the root pass.
pub(crate) fn append_widget_click_events(input: &mut egui::RawInput) {
    if input.viewport_id != egui::ViewportId::ROOT
        || input.events.iter().any(|event| {
            matches!(
                event,
                egui::Event::PointerMoved(_)
                    | egui::Event::PointerButton { .. }
                    | egui::Event::PointerGone
            )
        })
    {
        return;
    }
    WIDGET_CLICK_DRIVER.with(|driver| {
        let mut driver = driver.borrow_mut();
        let Some(click) = driver.active.as_mut() else {
            return;
        };
        let point = click.point;
        let pressed = matches!(click.phase, WidgetClickPhase::Down);
        input.events.push(egui::Event::PointerMoved(point));
        if !matches!(click.phase, WidgetClickPhase::Hover) {
            input.events.push(egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        match click.phase {
            WidgetClickPhase::Down => click.phase = WidgetClickPhase::Up,
            WidgetClickPhase::Up | WidgetClickPhase::Hover => driver.active = None,
        }
    });
}

fn widget_click_in_progress() -> bool {
    WIDGET_CLICK_DRIVER.with(|driver| {
        let driver = driver.borrow();
        driver.requested.is_some() || driver.active.is_some()
    })
}

/// Move the real Windows pointer through SendInput to a rect reported by the Details widget.
/// The runner worker performs the OS call; the UI thread only publishes geometry.
#[cfg(all(any(feature = "test-script", test), windows))]
fn send_real_pointer_move(hwnd_raw: u64, point: egui::Pos2, ppp: f32) -> Result<(), String> {
    use windows::Win32::Foundation::{HWND, POINT, RECT};
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        SetThreadDpiAwarenessContext,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE,
        MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, SendInput,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClientRect, GetSystemMetrics, IsWindowVisible, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
        SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    };

    struct DpiGuard(DPI_AWARENESS_CONTEXT);
    impl Drop for DpiGuard {
        fn drop(&mut self) {
            unsafe { SetThreadDpiAwarenessContext(self.0) };
        }
    }
    if !point.x.is_finite() || !point.y.is_finite() || !ppp.is_finite() || ppp <= 0.0 {
        return Err("Details preview pointer geometry is invalid".into());
    }
    let previous =
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    if previous.0.is_null() {
        return Err("SetThreadDpiAwarenessContext failed for Details pointer move".into());
    }
    let _dpi = DpiGuard(previous);
    let hwnd = HWND(hwnd_raw as usize as *mut _);
    if !unsafe { IsWindowVisible(hwnd).as_bool() } {
        return Err("root host is hidden before Details pointer move".into());
    }
    let mut client = RECT::default();
    unsafe { GetClientRect(hwnd, &mut client) }
        .map_err(|error| format!("GetClientRect failed for Details pointer move: {error}"))?;
    let mut target = POINT {
        x: (point.x * ppp).round() as i32,
        y: (point.y * ppp).round() as i32,
    };
    if target.x < client.left
        || target.x >= client.right
        || target.y < client.top
        || target.y >= client.bottom
    {
        return Err("Details preview cell is outside the root client area".into());
    }
    if !unsafe { ClientToScreen(hwnd, &mut target).as_bool() } {
        return Err("ClientToScreen failed for Details pointer move".into());
    }
    let left = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let top = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
    if width <= 1
        || height <= 1
        || target.x < left
        || target.x >= left + width
        || target.y < top
        || target.y >= top + height
    {
        return Err("Details preview cell is outside the virtual desktop".into());
    }
    let scaled = |value: i32, origin: i32, extent: i32| -> i32 {
        let numerator = i64::from(value - origin) * 65_535;
        ((numerator + i64::from(extent - 1) / 2) / i64::from(extent - 1)) as i32
    };
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: scaled(target.x, left, width),
                dy: scaled(target.y, top, height),
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let inserted = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    if inserted != 1 {
        return Err(format!(
            "SendInput inserted {inserted} of 1 Details pointer events"
        ));
    }
    crate::logger::log(format!(
        "[test-script] Details preview real pointer moved to screen=({}, {}) root=0x{hwnd_raw:x}",
        target.x, target.y
    ));
    Ok(())
}

#[cfg(all(any(feature = "test-script", test), not(windows)))]
fn send_real_pointer_move(_hwnd_raw: u64, _point: egui::Pos2, _ppp: f32) -> Result<(), String> {
    Err("Details preview real pointer move requires Windows".into())
}

pub(crate) fn take_smoke_action(action: UiSmokeAction) -> bool {
    let Ok(mut guard) = runtime().lock() else {
        return false;
    };
    let Some(runtime) = guard.as_mut() else {
        return false;
    };
    if runtime.smoke_actions.front() == Some(&action) {
        runtime.smoke_actions.pop_front();
        true
    } else {
        false
    }
}

#[derive(Default)]
struct InterruptState {
    failure: Mutex<Option<String>>,
    changed: Condvar,
}

impl InterruptState {
    fn fail(&self, message: impl Into<String>) {
        let Ok(mut guard) = self.failure.lock() else {
            return;
        };
        if guard.is_none() {
            *guard = Some(message.into());
            self.changed.notify_all();
        }
    }

    fn check(&self) -> Result<(), String> {
        self.failure
            .lock()
            .map_err(|_| "script interrupt state is poisoned".to_string())?
            .clone()
            .map_or(Ok(()), Err)
    }

    fn failure_message(&self) -> Option<String> {
        self.failure.lock().ok().and_then(|guard| guard.clone())
    }

    /// Sleep for `duration`, returning early only if the run has actually failed.
    ///
    /// The loop is the point. `wait_timeout` returns on spurious wakeups as well as on notify, so
    /// taking the first wakeup as "the wait is over" makes a hold last however long the condvar
    /// felt like: a twenty-second burst returned in 45ms, the script sailed past its assertions
    /// because nothing had happened yet, and the run reported success. A scenario that silently
    /// does not run is worse than one that fails.
    fn wait(&self, duration: Duration) -> Result<(), String> {
        let deadline = std::time::Instant::now() + duration;
        let mut guard = self
            .failure
            .lock()
            .map_err(|_| "script interrupt state is poisoned".to_string())?;
        loop {
            if let Some(message) = guard.as_ref() {
                return Err(message.clone());
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return Ok(());
            }
            let (next, _) = self
                .changed
                .wait_timeout(guard, deadline - now)
                .map_err(|_| "script interrupt state is poisoned".to_string())?;
            guard = next;
        }
    }
}

#[derive(Clone)]
struct RunnerBridge {
    tx: mpsc::Sender<UiCommand>,
    snapshot: Arc<RwLock<TestScriptSnapshot>>,
    interrupt: Arc<InterruptState>,
    wake: Arc<dyn Fn() + Send + Sync>,
    next_hold_id: Arc<AtomicU64>,
    action_selection: Arc<Mutex<TestScriptActionSelection>>,
    pointer_regions: pointer_input::SharedRegionCatalog,
}

impl RunnerBridge {
    fn send(&self, command: UiCommand) -> Result<(), String> {
        self.interrupt.check()?;
        self.send_unchecked(command)
    }

    fn send_unchecked(&self, command: UiCommand) -> Result<(), String> {
        self.tx
            .send(command)
            .map_err(|_| "test-script UI command channel disconnected".to_string())?;
        (self.wake)();
        Ok(())
    }

    fn latest_snapshot(&self) -> Result<TestScriptSnapshot, String> {
        self.snapshot
            .read()
            .map(|snapshot| snapshot.clone())
            .map_err(|_| "test-script snapshot is poisoned".to_string())
    }

    fn require_key_target(&self) -> Result<(), String> {
        let snapshot = self.latest_snapshot()?;
        let satisfied = snapshot.target_registered && snapshot.focused;
        let _ = self.send_unchecked(UiCommand::Precondition(PreconditionTrace {
            name: "key_target",
            satisfied,
            timeout_ms: None,
            elapsed_ms: 0,
            target_registered: Some(snapshot.target_registered),
            focused: Some(snapshot.focused),
        }));
        if !snapshot.target_registered {
            return Err(
                "synthetic key target is not registered; wait_until(|s| s.target_registered, timeout_ms) before sending input"
                    .to_string(),
            );
        }
        if !snapshot.focused {
            return Err(
                "synthetic key target is not focused; wait_until(|s| s.focused, timeout_ms) before sending input"
                    .to_string(),
            );
        }
        Ok(())
    }

    fn allocate_hold_id(&self) -> u64 {
        self.next_hold_id.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn action_selection(&self) -> Result<TestScriptActionSelection, String> {
        self.action_selection
            .lock()
            .map(|selection| selection.clone())
            .map_err(|_| "test-script action selection is poisoned".to_string())
    }

    fn capture(&self, label: &str, scope: CaptureScope) -> Result<(), String> {
        let selection = self.action_selection()?;
        let (reply, received) = mpsc::sync_channel(1);
        self.send(UiCommand::Capture {
            label: label.to_owned(),
            scope,
            selection,
            reply,
        })?;
        let deadline = Instant::now() + capture::EXPLICIT_TIMEOUT + Duration::from_secs(1);
        loop {
            self.interrupt.check()?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(format!(
                    "capture '{label}' timed out waiting for screenshot evidence"
                ));
            }
            match received.recv_timeout(remaining.min(Duration::from_millis(100))) {
                Ok(result) => return result,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("capture response channel disconnected".into());
                }
            }
        }
    }

    fn select_root(&self) -> Result<Map, String> {
        let snapshot = self.latest_snapshot()?;
        let window = snapshot
            .windows
            .iter()
            .find(|window| window.role == "root")
            .ok_or_else(|| "select_root could not find the root window".to_string())?;
        self.select_window_snapshot(window)
    }

    #[cfg(any(feature = "test-script", test))]
    fn move_details_preview_pointer(&self, timeout: Duration) -> Result<(), String> {
        if timeout.is_zero() {
            return Err("move_details_preview_pointer timeout_ms must be greater than zero".into());
        }
        let root = self
            .latest_snapshot()?
            .windows
            .into_iter()
            .find(|window| window.role == "root")
            .and_then(|window| window.identity)
            .ok_or("move_details_preview_pointer has no live root host")?;
        let (reply, received) = mpsc::sync_channel(1);
        self.send(UiCommand::DetailsPreviewPointer { reply })?;
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or("move_details_preview_pointer timeout is too large")?;
        let (point, pixels_per_point) = loop {
            self.interrupt.check()?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("timed out waiting for a visible Details preview cell".into());
            }
            match received.recv_timeout(remaining.min(WAIT_POLL_INTERVAL)) {
                Ok(result) => break result?,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("Details preview geometry channel disconnected".into());
                }
            }
        };
        if !eframe::miv_test_script_window_witness::is_current(
            egui::ViewportId::ROOT,
            root.hwnd(),
            root.backend_token(),
        )
        .map_err(str::to_owned)?
        {
            return Err("root host changed before Details pointer move".into());
        }
        send_real_pointer_move(root.hwnd(), point, pixels_per_point)
    }

    fn select_window(&self, window_id: u64, context_serial: u64) -> Result<Map, String> {
        let snapshot = self.latest_snapshot()?;
        let window = snapshot
            .windows
            .iter()
            .find(|window| {
                window.role == "detached"
                    && window.window_id == Some(window_id)
                    && window.context_serial == context_serial
            })
            .ok_or_else(|| {
                format!(
                    "select_window target is not current: window={window_id} context={context_serial}"
                )
            })?;
        self.select_window_snapshot(window)
    }

    fn select_window_snapshot(&self, window: &TestScriptWindowSnapshot) -> Result<Map, String> {
        let identity = window
            .identity
            .clone()
            .ok_or_else(|| format!("select_{} target has no current host identity", window.role))?;
        let mut selection = self
            .action_selection
            .lock()
            .map_err(|_| "test-script action selection is poisoned".to_string())?;
        *selection = TestScriptActionSelection::Targeted(identity.clone());
        drop(selection);

        let mut selected = window.to_rhai_map();
        selected.insert("target_mode".into(), Dynamic::from("targeted"));
        selected.insert("current".into(), Dynamic::from(true));
        let _ = self.send_unchecked(UiCommand::Log(format!(
            "action target selected mode=targeted owner={}",
            identity.describe()
        )));
        Ok(selected)
    }

    fn selected_target(&self) -> Result<Map, String> {
        let selection = self.action_selection()?;
        let snapshot = self.latest_snapshot()?;
        let mut selected = Map::new();
        selected.insert("target_mode".into(), Dynamic::from(selection.mode()));
        match selection {
            TestScriptActionSelection::LegacyImplicit => {
                selected.insert("current".into(), Dynamic::from(true));
                selected.insert("role".into(), Dynamic::from("implicit"));
            }
            TestScriptActionSelection::Targeted(identity) => {
                let current = snapshot
                    .windows
                    .iter()
                    .any(|window| window.identity.as_ref() == Some(&identity));
                selected.insert("current".into(), Dynamic::from(current));
                selected.insert("role".into(), Dynamic::from(identity.role()));
                selected.insert(
                    "context_serial".into(),
                    Dynamic::from(saturating_rhai_int(identity.context_serial())),
                );
                selected.insert(
                    "window_id".into(),
                    identity
                        .window_id()
                        .map(|value| Dynamic::from(saturating_rhai_int(value)))
                        .unwrap_or(Dynamic::UNIT),
                );
            }
        }
        Ok(selected)
    }

    fn selected_detached_identity(&self) -> Result<TestScriptWindowIdentity, String> {
        match self.action_selection()? {
            TestScriptActionSelection::Targeted(
                identity @ TestScriptWindowIdentity::Detached { .. },
            ) => Ok(identity),
            TestScriptActionSelection::Targeted(identity) => Err(format!(
                "native mouse input requires a detached target; selected {}",
                identity.describe()
            )),
            TestScriptActionSelection::LegacyImplicit => {
                Err("native mouse input requires select_window first".to_string())
            }
        }
    }

    fn validate_selected_owner_cached(
        &self,
        expected: &TestScriptWindowIdentity,
        deadline: Instant,
    ) -> Result<(), String> {
        if Instant::now() >= deadline {
            return Err("native mouse deadline expired while validating the selected owner".into());
        }
        self.interrupt.check()?;
        match self.action_selection()? {
            TestScriptActionSelection::Targeted(identity) if identity == *expected => {}
            _ => {
                return Err(format!(
                    "native mouse selected owner changed: expected {}",
                    expected.describe()
                ));
            }
        }
        if !self
            .latest_snapshot()?
            .windows
            .iter()
            .any(|window| window.identity.as_ref() == Some(expected))
        {
            return Err(format!(
                "native mouse selected owner is absent from the published snapshot: {}",
                expected.describe()
            ));
        }
        let backend_is_current = eframe::miv_test_script_window_witness::is_current(
            expected.viewport_id(),
            expected.hwnd(),
            expected.backend_token(),
        )
        .map_err(|error| {
            let message = format!("native window witness validation failed: {error}");
            self.interrupt.fail(message.clone());
            message
        })?;
        if !backend_is_current {
            return Err(format!(
                "native mouse backend allocation is no longer current: {}",
                expected.describe()
            ));
        }
        Ok(())
    }

    fn validate_selected_owner_fresh(
        &self,
        expected: &TestScriptWindowIdentity,
        deadline: Instant,
    ) -> Result<(), String> {
        self.validate_selected_owner_cached(expected, deadline)?;
        let (reply, acknowledgement) = mpsc::sync_channel(1);
        let command = UiCommand::ValidateSelectedOwner {
            expected_identity: expected.clone(),
            reply,
        };
        if self.tx.send(command).is_err() {
            let message = "selected-owner validation channel disconnected before dispatch";
            self.interrupt.fail(message);
            return Err(message.to_string());
        }
        (self.wake)();

        loop {
            self.interrupt.check()?;
            let now = Instant::now();
            if now >= deadline {
                return Err(format!(
                    "timed out waiting for fresh selected-owner validation: {}",
                    expected.describe()
                ));
            }
            let wait = WAIT_POLL_INTERVAL.min(deadline.saturating_duration_since(now));
            match acknowledgement.recv_timeout(wait) {
                Ok(result) => {
                    result?;
                    return self.validate_selected_owner_cached(expected, deadline);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let message = "selected-owner validation acknowledgement channel disconnected";
                    self.interrupt.fail(message);
                    return Err(message.to_string());
                }
            }
        }
    }

    #[cfg(feature = "test-script")]
    fn move_native_canvas(&self, normalized: [f32; 2], timeout: Duration) -> Result<Map, String> {
        if timeout.is_zero() {
            return Err("move_native_canvas timeout_ms must be greater than zero".to_string());
        }
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| "move_native_canvas timeout is too large".to_string())?;
        let identity = self.selected_detached_identity()?;
        let owner_hwnd = identity.hwnd();
        let prepared = crate::video::native_ui_smoke::prepare_real_mouse_in_canvas(
            owner_hwnd,
            normalized,
            deadline,
            || self.validate_selected_owner_fresh(&identity, deadline),
        )?;
        let receipt = crate::video::native_ui_smoke::send_prepared_real_mouse_move(
            prepared,
            deadline,
            || self.validate_selected_owner_fresh(&identity, deadline),
        )?;
        Ok(native_mouse_receipt_to_rhai_map(receipt))
    }

    #[cfg(feature = "test-script")]
    fn reveal_native_top_panorama_receipts(
        &self,
        timeout: Duration,
    ) -> Result<
        (
            crate::video::native_ui_smoke::NativeUiSmokeMoveReceipt,
            crate::video::native_ui_smoke::NativeUiSmokeNamedControlReceipt,
        ),
        String,
    > {
        if timeout.is_zero() {
            return Err(
                "reveal_native_top_panorama timeout_ms must be greater than zero".to_string(),
            );
        }
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| "reveal_native_top_panorama timeout is too large".to_string())?;
        let identity = self.selected_detached_identity()?;
        let owner_hwnd = identity.hwnd();
        let prepared = crate::video::native_ui_smoke::prepare_real_mouse_in_top_hover_activation(
            owner_hwnd,
            deadline,
            || self.validate_selected_owner_fresh(&identity, deadline),
        )?;
        let move_receipt = crate::video::native_ui_smoke::send_prepared_real_mouse_move(
            prepared,
            deadline,
            || self.validate_selected_owner_fresh(&identity, deadline),
        )?;
        let target_receipt =
            crate::video::native_ui_smoke::wait_for_native_top_panorama_after_move(
                &move_receipt,
                deadline,
                || self.validate_selected_owner_fresh(&identity, deadline),
            )?;
        Ok((move_receipt, target_receipt))
    }

    #[cfg(feature = "test-script")]
    fn reveal_native_top_panorama(&self, timeout: Duration) -> Result<Map, String> {
        let (move_receipt, target_receipt) = self.reveal_native_top_panorama_receipts(timeout)?;
        let mut result = Map::new();
        result.insert(
            "mouse".into(),
            Dynamic::from(native_mouse_receipt_to_rhai_map(move_receipt)),
        );
        result.insert(
            "target".into(),
            Dynamic::from(native_named_control_receipt_to_rhai_map(target_receipt)),
        );
        Ok(result)
    }

    #[cfg(feature = "test-script")]
    fn click_native_top_panorama(&self, timeout: Duration) -> Result<Map, String> {
        if timeout.is_zero() {
            return Err(
                "click_native_top_panorama timeout_ms must be greater than zero".to_string(),
            );
        }
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| "click_native_top_panorama timeout is too large".to_string())?;
        let identity = self.selected_detached_identity()?;
        let host_incarnation = identity
            .host_incarnation()
            .ok_or_else(|| "native button target has no detached host incarnation".to_string())?;
        let backend_token = identity.backend_token();
        let remaining = deadline.saturating_duration_since(Instant::now());
        let (move_receipt, target_receipt) = self.reveal_native_top_panorama_receipts(remaining)?;
        self.validate_selected_owner_fresh(&identity, deadline)?;
        let prepared = crate::video::native_ui_smoke::prepare_native_top_panorama_click(
            &target_receipt,
            host_incarnation,
            backend_token,
            deadline,
            || self.validate_selected_owner_fresh(&identity, deadline),
        )?;
        let click_receipt = crate::video::native_ui_smoke::click_prepared_native_top_panorama(
            prepared,
            deadline,
            || self.validate_selected_owner_fresh(&identity, deadline),
        )?;
        let mut result = Map::new();
        result.insert(
            "mouse".into(),
            Dynamic::from(native_mouse_receipt_to_rhai_map(move_receipt)),
        );
        result.insert(
            "target".into(),
            Dynamic::from(native_named_control_receipt_to_rhai_map(target_receipt)),
        );
        result.insert(
            "click".into(),
            Dynamic::from(native_top_panorama_click_receipt_to_rhai_map(click_receipt)),
        );
        Ok(result)
    }
}

#[cfg(feature = "test-script")]
fn native_mouse_environment_error(bridge: &RunnerBridge, message: String) -> Box<EvalAltResult> {
    bridge.interrupt.fail(message.clone());
    rhai_error(message)
}

#[cfg(feature = "test-script")]
fn native_mouse_receipt_to_rhai_map(
    receipt: crate::video::native_ui_smoke::NativeUiSmokeMoveReceipt,
) -> Map {
    let mut map = Map::new();
    map.insert("token".into(), saturating_rhai_int(receipt.token).into());
    map.insert(
        "owner_hwnd".into(),
        Dynamic::from(format!("0x{:x}", receipt.owner_hwnd)),
    );
    map.insert(
        "presenter_hwnd".into(),
        Dynamic::from(format!("0x{:x}", receipt.presenter_hwnd)),
    );
    map.insert(
        "source_epoch".into(),
        saturating_rhai_int(receipt.source_epoch).into(),
    );
    map.insert(
        "generation".into(),
        saturating_rhai_int(receipt.generation).into(),
    );
    map.insert(
        "requested_client_x".into(),
        rhai::INT::from(receipt.requested_client_x).into(),
    );
    map.insert(
        "requested_client_y".into(),
        rhai::INT::from(receipt.requested_client_y).into(),
    );
    map.insert(
        "actual_client_x".into(),
        rhai::INT::from(receipt.actual_client_x).into(),
    );
    map.insert(
        "actual_client_y".into(),
        rhai::INT::from(receipt.actual_client_y).into(),
    );
    map
}

#[cfg(feature = "test-script")]
fn native_named_control_receipt_to_rhai_map(
    receipt: crate::video::native_ui_smoke::NativeUiSmokeNamedControlReceipt,
) -> Map {
    let mut map = Map::new();
    map.insert("name".into(), Dynamic::from("native_top_panorama"));
    map.insert("token".into(), saturating_rhai_int(receipt.token).into());
    map.insert(
        "owner_hwnd".into(),
        Dynamic::from(format!("0x{:x}", receipt.owner_hwnd)),
    );
    map.insert(
        "presenter_hwnd".into(),
        Dynamic::from(format!("0x{:x}", receipt.presenter_hwnd)),
    );
    map.insert(
        "source_epoch".into(),
        saturating_rhai_int(receipt.source_epoch).into(),
    );
    map.insert(
        "generation".into(),
        saturating_rhai_int(receipt.generation).into(),
    );
    map.insert("client_x".into(), rhai::INT::from(receipt.client_x).into());
    map.insert("client_y".into(), rhai::INT::from(receipt.client_y).into());
    map.insert(
        "pixels_per_point".into(),
        rhai::FLOAT::from(receipt.pixels_per_point).into(),
    );
    for (prefix, rect) in [
        ("rect", receipt.rect),
        ("interact_rect", receipt.interact_rect),
        ("clip_rect", receipt.clip_rect),
    ] {
        map.insert(
            format!("{prefix}_min_x").into(),
            rhai::FLOAT::from(rect.min.x).into(),
        );
        map.insert(
            format!("{prefix}_min_y").into(),
            rhai::FLOAT::from(rect.min.y).into(),
        );
        map.insert(
            format!("{prefix}_max_x").into(),
            rhai::FLOAT::from(rect.max.x).into(),
        );
        map.insert(
            format!("{prefix}_max_y").into(),
            rhai::FLOAT::from(rect.max.y).into(),
        );
    }
    map.insert(
        "layer_id".into(),
        Dynamic::from(format!("{:?}", receipt.layer_id)),
    );
    map.insert("senses_click".into(), receipt.senses_click.into());
    map.insert("enabled".into(), receipt.enabled.into());
    map.insert(
        "panorama_pose_present".into(),
        receipt.panorama_pose_present.into(),
    );
    map.insert(
        "video_zoom_present".into(),
        receipt.video_zoom_present.into(),
    );
    let classification = match receipt.classification {
        crate::video::native_ui_smoke::NativeUiSmokePanoramaClassification::Unknown => "unknown",
        crate::video::native_ui_smoke::NativeUiSmokePanoramaClassification::Panorama => "panorama",
        crate::video::native_ui_smoke::NativeUiSmokePanoramaClassification::NonPanorama => {
            "non_panorama"
        }
    };
    map.insert("classification".into(), Dynamic::from(classification));
    map
}

#[cfg(feature = "test-script")]
fn native_button_delivery_to_rhai_map(
    delivery: crate::video::native_ui_smoke::NativeUiSmokeButtonDelivery,
) -> Map {
    let mut map = Map::new();
    map.insert("token".into(), saturating_rhai_int(delivery.token).into());
    map.insert(
        "receiver_hwnd".into(),
        Dynamic::from(format!("0x{:x}", delivery.receiver_hwnd)),
    );
    map.insert(
        "receiver_process_id".into(),
        saturating_rhai_int(u64::from(delivery.receiver_process_id)).into(),
    );
    map.insert(
        "receiver_thread_id".into(),
        saturating_rhai_int(u64::from(delivery.receiver_thread_id)).into(),
    );
    map.insert(
        "actual_client_x".into(),
        rhai::INT::from(delivery.actual_client_x).into(),
    );
    map.insert(
        "actual_client_y".into(),
        rhai::INT::from(delivery.actual_client_y).into(),
    );
    map.insert(
        "actual_screen_x".into(),
        rhai::INT::from(delivery.actual_screen_x).into(),
    );
    map.insert(
        "actual_screen_y".into(),
        rhai::INT::from(delivery.actual_screen_y).into(),
    );
    map
}

#[cfg(feature = "test-script")]
fn native_top_panorama_click_receipt_to_rhai_map(
    receipt: crate::video::native_ui_smoke::NativeUiSmokeTopPanoramaClickReceipt,
) -> Map {
    let mut map = Map::new();
    map.insert(
        "gesture_id".into(),
        saturating_rhai_int(receipt.gesture_id).into(),
    );
    map.insert(
        "down".into(),
        Dynamic::from(native_button_delivery_to_rhai_map(receipt.down)),
    );
    map.insert(
        "up".into(),
        Dynamic::from(native_button_delivery_to_rhai_map(receipt.up)),
    );
    map.insert(
        "app_video_zoom_scale".into(),
        rhai::FLOAT::from(receipt.app_video_zoom_scale).into(),
    );
    map.insert(
        "app_panorama_active".into(),
        receipt.app_panorama_active.into(),
    );
    map.insert("pointer_released".into(), receipt.pointer_released.into());
    map.insert(
        "drag_latches_released".into(),
        receipt.drag_latches_released.into(),
    );
    map
}

fn emit_perf_step(message: &str) {
    if !crate::perf::is_enabled() {
        return;
    }
    crate::perf::event(
        "test_script",
        "step",
        None,
        0,
        &[("message", serde_json::Value::from(message))],
    );
}

fn emit_perf_precondition(trace: &PreconditionTrace) {
    if !crate::perf::is_enabled() {
        return;
    }
    let mut extras = vec![
        ("name", serde_json::Value::from(trace.name)),
        ("satisfied", serde_json::Value::from(trace.satisfied)),
        ("elapsed_ms", serde_json::Value::from(trace.elapsed_ms)),
    ];
    if let Some(timeout_ms) = trace.timeout_ms {
        extras.push(("timeout_ms", serde_json::Value::from(timeout_ms)));
    }
    if let Some(target_registered) = trace.target_registered {
        extras.push((
            "target_registered",
            serde_json::Value::from(target_registered),
        ));
    }
    if let Some(focused) = trace.focused {
        extras.push(("focused", serde_json::Value::from(focused)));
    }
    crate::perf::event("test_script", "precondition", None, 0, &extras);
}

fn emit_perf_fail(outcome: &ScriptOutcome) {
    if outcome.kind == ScriptOutcomeKind::Success || !crate::perf::is_enabled() {
        return;
    }
    crate::perf::event(
        "test_script",
        "fail",
        None,
        0,
        &[
            (
                "failure_kind",
                serde_json::Value::from(outcome.kind.as_str()),
            ),
            (
                "exit_code",
                serde_json::Value::from(outcome.kind.exit_code()),
            ),
            ("message", serde_json::Value::from(outcome.message.as_str())),
        ],
    );
}

fn emit_perf_level_reads(observations: &[KeymapLevelObservation]) {
    if !crate::perf::is_enabled() {
        return;
    }
    for observation in observations {
        for hold_id in &observation.hold_ids {
            crate::perf::event(
                "test_script",
                "level_read",
                Some(&observation.key),
                0,
                &[
                    ("hold_id", serde_json::Value::from(*hold_id)),
                    ("held", serde_json::Value::from(observation.held)),
                    ("frame_nr", serde_json::Value::from(observation.frame_nr)),
                    ("reader", serde_json::Value::from("Keymap::key_held_chord")),
                ],
            );
        }
    }
}

fn rhai_error(message: impl Into<String>) -> Box<EvalAltResult> {
    EvalAltResult::ErrorRuntime(Dynamic::from(message.into()), rhai::Position::NONE).into()
}

fn checked_duration(ms: rhai::INT, argument: &str) -> Result<Duration, Box<EvalAltResult>> {
    let ms =
        u64::try_from(ms).map_err(|_| rhai_error(format!("{argument} must be zero or greater")))?;
    Ok(Duration::from_millis(ms))
}

fn checked_u64(value: rhai::INT, argument: &str) -> Result<u64, Box<EvalAltResult>> {
    u64::try_from(value).map_err(|_| rhai_error(format!("{argument} must be zero or greater")))
}

fn parse_navigation_key(name: &str) -> Result<SyntheticNavigationKey, Box<EvalAltResult>> {
    let key = match name.trim().to_ascii_lowercase().as_str() {
        "right" | "arrowright" => SyntheticNavigationKey::Right,
        "left" | "arrowleft" => SyntheticNavigationKey::Left,
        "up" | "arrowup" => SyntheticNavigationKey::Up,
        "down" | "arrowdown" => SyntheticNavigationKey::Down,
        "pageup" => SyntheticNavigationKey::PageUp,
        "pagedown" => SyntheticNavigationKey::PageDown,
        "home" => SyntheticNavigationKey::Home,
        "end" => SyntheticNavigationKey::End,
        "enter" => SyntheticNavigationKey::Enter,
        "escape" | "esc" => SyntheticNavigationKey::Escape,
        "f12" => SyntheticNavigationKey::F12,
        _ => {
            return Err(rhai_error(format!(
                "unsupported synthetic navigation key: {name}"
            )));
        }
    };
    Ok(key)
}

/// Parse a modifier spec such as `"ctrl"`, `"ctrl+shift"` or `""`.
///
/// Ctrl is what makes folder navigation (Ctrl+Up/Down) expressible, and the timeline already
/// answers `GetAsyncKeyState` for the modifier VKs from the held set, so a scripted Ctrl chord
/// reaches both the keymap's OS-level reads and egui's event modifiers - the same two
/// representations a physical press produces.
fn parse_modifiers(spec: &str) -> Result<SyntheticModifiers, Box<EvalAltResult>> {
    let mut modifiers = SyntheticModifiers::default();
    let spec = spec.trim();
    if spec.is_empty() || spec.eq_ignore_ascii_case("none") {
        return Ok(modifiers);
    }
    for part in spec.split(['+', ',']) {
        match part.trim().to_ascii_lowercase().as_str() {
            "" => {}
            "ctrl" | "control" => modifiers.ctrl = true,
            "shift" => modifiers.shift = true,
            "alt" => modifiers.alt = true,
            other => {
                return Err(rhai_error(format!(
                    "unsupported synthetic modifier: {other} (in {spec:?})"
                )));
            }
        }
    }
    Ok(modifiers)
}

fn hold_key_impl(
    bridge: &RunnerBridge,
    name: &str,
    modifiers: SyntheticModifiers,
    ms: rhai::INT,
) -> Result<(), Box<EvalAltResult>> {
    let key = parse_navigation_key(name)?;
    let duration = checked_duration(ms, "hold_key ms")?;
    bridge.require_key_target().map_err(rhai_error)?;
    let hold_id = bridge.allocate_hold_id();
    bridge
        .send(UiCommand::Key(
            SyntheticKeyCommand::down(Instant::now(), key, modifiers).with_hold_id(hold_id),
        ))
        .map_err(rhai_error)?;
    if let Err(error) = wait_interruptibly(&bridge.interrupt, duration) {
        let _ = bridge.send_unchecked(UiCommand::Key(
            SyntheticKeyCommand::up(Instant::now(), key).with_hold_id(hold_id),
        ));
        return Err(error);
    }
    bridge
        .send(UiCommand::Key(
            SyntheticKeyCommand::up(Instant::now(), key).with_hold_id(hold_id),
        ))
        .map_err(rhai_error)
}

fn tap_key_impl(
    bridge: &RunnerBridge,
    name: &str,
    modifiers: SyntheticModifiers,
) -> Result<(), Box<EvalAltResult>> {
    let key = parse_navigation_key(name)?;
    bridge.require_key_target().map_err(rhai_error)?;
    bridge
        .send(UiCommand::Key(SyntheticKeyCommand::down(
            Instant::now(),
            key,
            modifiers,
        )))
        .map_err(rhai_error)?;
    bridge
        .send(UiCommand::Key(SyntheticKeyCommand::up(Instant::now(), key)))
        .map_err(rhai_error)
}

fn parse_action(name: &str) -> Result<KeyAction, Box<EvalAltResult>> {
    let action = KeyAction::from_ini_name(name)
        .ok_or_else(|| rhai_error(format!("unknown KeyAction ini name: {name}")))?;
    if action.trigger() != KeyTrigger::Press {
        return Err(rhai_error(format!(
            "run_action only accepts one-shot Press actions: {name}"
        )));
    }
    Ok(action)
}

fn wait_interruptibly(
    interrupt: &InterruptState,
    duration: Duration,
) -> Result<(), Box<EvalAltResult>> {
    interrupt.wait(duration).map_err(rhai_error)
}

fn register_runner_api(engine: &mut Engine, bridge: RunnerBridge) {
    let always_on_top_bridge = bridge.clone();
    engine.register_fn(
        "always_on_top_smoke",
        move |name: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            let action = match name.as_str() {
                "on" => UiSmokeAction::AlwaysOnTopOn,
                "off" => UiSmokeAction::AlwaysOnTopOff,
                "minimize_root" => UiSmokeAction::MinimizeRoot,
                "restore_root" => UiSmokeAction::RestoreRoot,
                "hide_to_tray" => UiSmokeAction::HideToTray,
                "restore_from_tray" => UiSmokeAction::RestoreFromTray,
                "close_fullscreen" => UiSmokeAction::CloseFullscreen,
                "toggle_detached_mode" => UiSmokeAction::ToggleDetachedMode,
                "enable_independent_windows" => UiSmokeAction::EnableIndependentWindows,
                _ => {
                    return Err(rhai_error(format!(
                        "unknown always-on-top smoke action: {name}"
                    )));
                }
            };
            always_on_top_bridge
                .send(UiCommand::SmokeAction(action))
                .map_err(rhai_error)
        },
    );
    let rating_sort_bridge = bridge.clone();
    engine.register_fn(
        "rating_sort_smoke",
        move |name: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            let action = match name.as_str() {
                "open_thumbnail_preferences" => UiSmokeAction::OpenThumbnailPreferences,
                "open_first_smart_folder" => UiSmokeAction::OpenFirstSmartFolder,
                _ => {
                    return Err(rhai_error(format!(
                        "unknown rating-sort smoke action: {name}"
                    )));
                }
            };
            rating_sort_bridge
                .send(UiCommand::SmokeAction(action))
                .map_err(rhai_error)
        },
    );
    let collection_sort_bridge = bridge.clone();
    engine.register_fn(
        "collection_sort_smoke",
        move |name: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            match name.as_str() {
                "open_seeded_collection" => collection_sort_bridge
                    .send(UiCommand::SmokeAction(UiSmokeAction::OpenSeededCollection))
                    .map_err(rhai_error),
                _ => Err(rhai_error(format!(
                    "unknown Collection sort smoke action: {name}"
                ))),
            }
        },
    );
    let click_widget_bridge = bridge.clone();
    engine.register_fn(
        "click_widget",
        move |label: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            let (reply, received) = mpsc::sync_channel(1);
            click_widget_bridge
                .send(UiCommand::ClickWidget {
                    label: label.to_string(),
                    reply,
                })
                .map_err(rhai_error)?;
            let started = Instant::now();
            loop {
                click_widget_bridge.interrupt.check().map_err(rhai_error)?;
                if started.elapsed() >= Duration::from_secs(30) {
                    return Err(rhai_error(format!(
                        "click_widget timed out waiting for visible widget: {label}"
                    )));
                }
                match received.recv_timeout(WAIT_POLL_INTERVAL) {
                    Ok(Ok(())) => return Ok(()),
                    Ok(Err(message)) => return Err(rhai_error(message)),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        return Err(rhai_error("widget click acknowledgement disconnected"));
                    }
                }
            }
        },
    );
    for (name, kind) in [
        ("hover_sort_row", SortPopupPointerKind::Hover),
        ("click_sort_row", SortPopupPointerKind::Click),
    ] {
        let sort_pointer_bridge = bridge.clone();
        engine.register_fn(
            name,
            move |label: ImmutableString| -> Result<(), Box<EvalAltResult>> {
                let (reply, received) = mpsc::sync_channel(1);
                sort_pointer_bridge
                    .send(UiCommand::SortPopupPointer {
                        label: label.to_string(),
                        kind,
                        reply,
                    })
                    .map_err(rhai_error)?;
                let started = Instant::now();
                loop {
                    sort_pointer_bridge.interrupt.check().map_err(rhai_error)?;
                    if started.elapsed() >= Duration::from_secs(10) {
                        return Err(rhai_error(format!(
                            "{name} timed out waiting for visible sort row: {label}"
                        )));
                    }
                    match received.recv_timeout(WAIT_POLL_INTERVAL) {
                        Ok(Ok(())) => return Ok(()),
                        Ok(Err(message)) => return Err(rhai_error(message)),
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            return Err(rhai_error(
                                "sort row pointer acknowledgement disconnected",
                            ));
                        }
                    }
                }
            },
        );
    }
    // Keep the opt-in production API available to unit tests as well, so the
    // ordinary lib suite exercises its registration and missing-host error.
    #[cfg(any(feature = "test-script", test))]
    {
        let preview_bridge = bridge.clone();
        engine.register_fn(
            "move_details_preview_pointer",
            move |timeout_ms: rhai::INT| -> Result<(), Box<EvalAltResult>> {
                preview_bridge
                    .move_details_preview_pointer(checked_duration(
                        timeout_ms,
                        "move_details_preview_pointer timeout_ms",
                    )?)
                    .map_err(rhai_error)
            },
        );
    }
    let select_root_bridge = bridge.clone();
    engine.register_fn("select_root", move || -> Result<Map, Box<EvalAltResult>> {
        select_root_bridge.select_root().map_err(rhai_error)
    });

    let select_window_bridge = bridge.clone();
    engine.register_fn(
        "select_window",
        move |window_id: rhai::INT, context_serial: rhai::INT| -> Result<Map, Box<EvalAltResult>> {
            select_window_bridge
                .select_window(
                    checked_u64(window_id, "select_window window_id")?,
                    checked_u64(context_serial, "select_window context_serial")?,
                )
                .map_err(rhai_error)
        },
    );

    let selected_target_bridge = bridge.clone();
    engine.register_fn(
        "selected_target",
        move || -> Result<Map, Box<EvalAltResult>> {
            selected_target_bridge.selected_target().map_err(rhai_error)
        },
    );

    #[cfg(feature = "test-script")]
    {
        let native_mouse_bridge = bridge.clone();
        engine.register_fn(
            "move_native_canvas",
            move |normalized_x: rhai::FLOAT,
                  normalized_y: rhai::FLOAT,
                  timeout_ms: rhai::INT|
                  -> Result<Map, Box<EvalAltResult>> {
                if !normalized_x.is_finite()
                    || !normalized_y.is_finite()
                    || normalized_x <= 0.0
                    || normalized_x >= 1.0
                    || normalized_y <= 0.0
                    || normalized_y >= 1.0
                {
                    return Err(rhai_error(
                        "move_native_canvas coordinates must be finite and between zero and one",
                    ));
                }
                let timeout = checked_duration(timeout_ms, "move_native_canvas timeout_ms")?;
                if timeout.is_zero() {
                    return Err(rhai_error(
                        "move_native_canvas timeout_ms must be greater than zero",
                    ));
                }
                native_mouse_bridge
                    .move_native_canvas([normalized_x as f32, normalized_y as f32], timeout)
                    .map_err(|message| {
                        native_mouse_environment_error(&native_mouse_bridge, message)
                    })
            },
        );

        let native_top_panorama_bridge = bridge.clone();
        engine.register_fn(
            "reveal_native_top_panorama",
            move |timeout_ms: rhai::INT| -> Result<Map, Box<EvalAltResult>> {
                let timeout =
                    checked_duration(timeout_ms, "reveal_native_top_panorama timeout_ms")?;
                if timeout.is_zero() {
                    return Err(rhai_error(
                        "reveal_native_top_panorama timeout_ms must be greater than zero",
                    ));
                }
                native_top_panorama_bridge
                    .reveal_native_top_panorama(timeout)
                    .map_err(|message| {
                        native_mouse_environment_error(&native_top_panorama_bridge, message)
                    })
            },
        );

        let native_top_panorama_click_bridge = bridge.clone();
        engine.register_fn(
            "click_native_top_panorama",
            move |timeout_ms: rhai::INT| -> Result<Map, Box<EvalAltResult>> {
                let timeout = checked_duration(timeout_ms, "click_native_top_panorama timeout_ms")?;
                if timeout.is_zero() {
                    return Err(rhai_error(
                        "click_native_top_panorama timeout_ms must be greater than zero",
                    ));
                }
                native_top_panorama_click_bridge
                    .click_native_top_panorama(timeout)
                    .map_err(|message| {
                        native_mouse_environment_error(&native_top_panorama_click_bridge, message)
                    })
            },
        );
    }

    let hold_bridge = bridge.clone();
    engine.register_fn(
        "hold_key",
        move |name: ImmutableString, ms: rhai::INT| -> Result<(), Box<EvalAltResult>> {
            hold_key_impl(&hold_bridge, &name, SyntheticModifiers::default(), ms)
        },
    );

    // `hold_key("Down", "ctrl", 3000)` - folder navigation is a Ctrl chord, so without this
    // overload the harness cannot express the input that both of the v3.0.0 fullscreen defects
    // start from.
    let hold_mod_bridge = bridge.clone();
    engine.register_fn(
        "hold_key",
        move |name: ImmutableString,
              modifiers: ImmutableString,
              ms: rhai::INT|
              -> Result<(), Box<EvalAltResult>> {
            let modifiers = parse_modifiers(&modifiers)?;
            hold_key_impl(&hold_mod_bridge, &name, modifiers, ms)
        },
    );

    let tap_bridge = bridge.clone();
    engine.register_fn(
        "tap_key",
        move |name: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            tap_key_impl(&tap_bridge, &name, SyntheticModifiers::default())
        },
    );

    let tap_mod_bridge = bridge.clone();
    engine.register_fn(
        "tap_key",
        move |name: ImmutableString,
              modifiers: ImmutableString|
              -> Result<(), Box<EvalAltResult>> {
            let modifiers = parse_modifiers(&modifiers)?;
            tap_key_impl(&tap_mod_bridge, &name, modifiers)
        },
    );

    let release_bridge = bridge.clone();
    engine.register_fn(
        "release_key",
        move |name: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            let key = parse_navigation_key(&name)?;
            release_bridge
                .send(UiCommand::Key(SyntheticKeyCommand::up(Instant::now(), key)))
                .map_err(rhai_error)
        },
    );

    let action_bridge = bridge.clone();
    engine.register_fn(
        "run_action",
        move |name: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            let action = parse_action(&name)?;
            let selection = action_bridge.action_selection().map_err(rhai_error)?;
            let (applied_tx, applied_rx) = mpsc::sync_channel(1);
            action_bridge
                .send(UiCommand::RunAction {
                    action,
                    selection,
                    applied: applied_tx,
                })
                .map_err(rhai_error)?;
            loop {
                action_bridge.interrupt.check().map_err(rhai_error)?;
                match applied_rx.recv_timeout(WAIT_POLL_INTERVAL) {
                    Ok(Ok(())) => return Ok(()),
                    Ok(Err(message)) => return Err(rhai_error(message)),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        return Err(rhai_error("run_action apply acknowledgement disconnected"));
                    }
                }
            }
        },
    );

    let sleep_bridge = bridge.clone();
    engine.register_fn(
        "sleep",
        move |ms: rhai::INT| -> Result<(), Box<EvalAltResult>> {
            wait_interruptibly(&sleep_bridge.interrupt, checked_duration(ms, "sleep ms")?)
        },
    );

    let wait_bridge = bridge.clone();
    engine.register_fn(
        "wait_until",
        move |ctx: NativeCallContext,
              condition: FnPtr,
              timeout_ms: rhai::INT|
              -> Result<(), Box<EvalAltResult>> {
            let timeout = checked_duration(timeout_ms, "wait_until timeout_ms")?;
            let started = Instant::now();
            loop {
                wait_bridge.interrupt.check().map_err(rhai_error)?;
                let snapshot = wait_bridge.latest_snapshot().map_err(rhai_error)?;
                if condition.call_within_context::<bool>(&ctx, (snapshot.to_rhai_map(),))? {
                    wait_bridge
                        .send(UiCommand::Precondition(PreconditionTrace {
                            name: "wait_until",
                            satisfied: true,
                            timeout_ms: Some(timeout.as_millis() as u64),
                            elapsed_ms: started.elapsed().as_millis().min(u128::from(u64::MAX))
                                as u64,
                            target_registered: Some(snapshot.target_registered),
                            focused: Some(snapshot.focused),
                        }))
                        .map_err(rhai_error)?;
                    return Ok(());
                }
                let elapsed = started.elapsed();
                if elapsed >= timeout {
                    let _ =
                        wait_bridge.send_unchecked(UiCommand::Precondition(PreconditionTrace {
                            name: "wait_until",
                            satisfied: false,
                            timeout_ms: Some(timeout.as_millis() as u64),
                            elapsed_ms: elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
                            target_registered: Some(snapshot.target_registered),
                            focused: Some(snapshot.focused),
                        }));
                    return Err(rhai_error(format!(
                        "wait_until timed out after {} ms",
                        timeout.as_millis()
                    )));
                }
                wait_interruptibly(
                    &wait_bridge.interrupt,
                    WAIT_POLL_INTERVAL.min(timeout.saturating_sub(elapsed)),
                )?;
            }
        },
    );

    // 条件を待つのではなく、いまの状態をそのまま読む。`wait_until` は真になるまで
    // 待つので、「どちらに転んだか」で分岐するシナリオが書けなかった。
    let snapshot_bridge = bridge.clone();
    engine.register_fn("snapshot", move || -> Result<Map, Box<EvalAltResult>> {
        let snapshot = snapshot_bridge.latest_snapshot().map_err(rhai_error)?;
        Ok(snapshot.to_rhai_map())
    });

    let repeat_float_bridge = bridge.clone();
    engine.register_fn(
        "set_repeat",
        move |delay_ms: rhai::INT, hz: rhai::FLOAT| -> Result<(), Box<EvalAltResult>> {
            let delay = checked_duration(delay_ms, "set_repeat delay_ms")?;
            if !hz.is_finite() || hz <= 0.0 {
                return Err(rhai_error(
                    "set_repeat hz must be finite and greater than zero",
                ));
            }
            repeat_float_bridge
                .send(UiCommand::SetRepeat { delay, hz })
                .map_err(rhai_error)
        },
    );

    let repeat_int_bridge = bridge.clone();
    engine.register_fn(
        "set_repeat",
        move |delay_ms: rhai::INT, hz: rhai::INT| -> Result<(), Box<EvalAltResult>> {
            let delay = checked_duration(delay_ms, "set_repeat delay_ms")?;
            if hz <= 0 {
                return Err(rhai_error("set_repeat hz must be greater than zero"));
            }
            repeat_int_bridge
                .send(UiCommand::SetRepeat {
                    delay,
                    hz: hz as f64,
                })
                .map_err(rhai_error)
        },
    );

    let log_bridge = bridge.clone();
    engine.register_fn(
        "log",
        move |message: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            log_bridge
                .send(UiCommand::Log(message.to_string()))
                .map_err(rhai_error)
        },
    );

    let capture_bridge = bridge.clone();
    engine.register_fn(
        "capture",
        move |label: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            capture_bridge
                .capture(&label, CaptureScope::Selected)
                .map_err(rhai_error)
        },
    );
    let capture_all_bridge = bridge.clone();
    engine.register_fn(
        "capture",
        move |label: ImmutableString, scope: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            let scope = match scope.as_str() {
                "current" => CaptureScope::Selected,
                "all" => CaptureScope::All,
                _ => return Err(rhai_error("capture scope must be 'current' or 'all'")),
            };
            capture_all_bridge
                .capture(&label, scope)
                .map_err(rhai_error)
        },
    );

    engine.register_fn(
        "fail",
        move |message: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            Err(rhai_error(format!("script fail: {message}")))
        },
    );
}

fn build_engine(bridge: RunnerBridge) -> Engine {
    let mut engine = Engine::new();
    engine.set_max_operations(100_000_000);
    engine.set_max_call_levels(64);
    engine.set_max_expr_depths(64, 64);
    engine.set_max_string_size(16 * 1024 * 1024);
    engine.set_max_array_size(1_000_000);
    engine.set_max_map_size(1_000_000);
    engine.disable_symbol("eval");
    engine.disable_symbol("import");

    let interrupt = Arc::clone(&bridge.interrupt);
    engine.on_progress(move |operations| {
        if operations & 0xFFFF == 0
            && let Err(message) = interrupt.check()
        {
            Some(Dynamic::from(message))
        } else {
            None
        }
    });
    pointer_input::register(
        &mut engine,
        bridge.clone(),
        Arc::clone(&bridge.pointer_regions),
    );
    register_runner_api(&mut engine, bridge);
    engine
}

fn evaluate_source(source: &str, bridge: RunnerBridge) -> ScriptOutcome {
    let engine = build_engine(bridge.clone());
    match engine.eval::<Dynamic>(source) {
        Ok(_) => bridge
            .interrupt
            .failure_message()
            .map(ScriptOutcome::environment_failure)
            .unwrap_or_else(ScriptOutcome::success),
        Err(error) => bridge
            .interrupt
            .failure_message()
            .map(ScriptOutcome::environment_failure)
            .unwrap_or_else(|| ScriptOutcome::script_failure(error.to_string())),
    }
}

fn load_script(path: &Path) -> Result<String, String> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("failed to inspect test script {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("test script is not a file: {}", path.display()));
    }
    if metadata.len() > MAX_SCRIPT_BYTES {
        return Err(format!(
            "test script exceeds {} bytes: {}",
            MAX_SCRIPT_BYTES,
            path.display()
        ));
    }
    std::fs::read_to_string(path)
        .map_err(|error| format!("failed to read test script {}: {error}", path.display()))
}

fn finish_worker(bridge: &RunnerBridge, outcome: ScriptOutcome) {
    let _ = bridge.send_unchecked(UiCommand::Cancel(Instant::now()));
    let _ = bridge.send_unchecked(UiCommand::Finished(outcome));
}

fn spawn_script_path(path: PathBuf, bridge: RunnerBridge) -> Result<(), String> {
    std::thread::Builder::new()
        .name("test-script-runner".to_string())
        .spawn(move || {
            let outcome = match load_script(&path) {
                Ok(source) => evaluate_source(&source, bridge.clone()),
                Err(error) => ScriptOutcome::script_failure(error),
            };
            finish_worker(&bridge, outcome);
        })
        .map(|_| ())
        .map_err(|error| format!("failed to spawn test-script runner: {error}"))
}

#[cfg(test)]
fn spawn_script_source(source: String, bridge: RunnerBridge) -> Result<(), String> {
    std::thread::Builder::new()
        .name("test-script-runner-test".to_string())
        .spawn(move || {
            let outcome = evaluate_source(&source, bridge.clone());
            finish_worker(&bridge, outcome);
        })
        .map(|_| ())
        .map_err(|error| format!("failed to spawn test-script runner: {error}"))
}

struct PendingAction {
    action: KeyAction,
    dispatch: PendingActionDispatch,
    // `None` means a non-consuming pressed_action peek already acknowledged
    // the command. Keep the entry until the frame ends so later peeks observe
    // the same press, just like an egui input event.
    applied: Option<mpsc::SyncSender<Result<(), String>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PendingActionDispatch {
    LegacyImplicit,
    Targeted {
        owner: TestScriptWindowIdentity,
        phase: TargetedActionPhase,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TargetedActionPhase {
    AwaitingDetachedOwner,
    // Focus must be applied in an earlier backend pass than an action which may
    // open another host. Otherwise a late source Focus steals that host's focus.
    AwaitingFocus,
    AwaitingPass,
}

#[derive(Clone, Debug)]
struct TestScriptActionPassObservation {
    pass: u64,
    owner: Option<TestScriptWindowIdentity>,
    eligible: bool,
}

fn joined_window_snapshots(
    authoritative: &[TestScriptWindowSnapshot],
    viewport_observations: &HashMap<TestScriptWindowIdentity, u64>,
    paint_observations: &HashMap<TestScriptPaintEvidenceKey, u64>,
    frame_paints: &HashMap<TestScriptWindowIdentity, Vec<TestScriptPaintObservation>>,
    sidecar_observations: &std::collections::HashSet<(u64, u64, TestScriptSidecarObservation)>,
) -> Vec<TestScriptWindowSnapshot> {
    authoritative
        .iter()
        .cloned()
        .map(|mut window| {
            if let Some(revision) = window
                .identity
                .as_ref()
                .and_then(|identity| viewport_observations.get(identity))
            {
                window.viewport_rendered = true;
                window.viewport_revision = *revision;
            }
            let best = paint_observations
                .iter()
                .filter(|(key, _)| {
                    window.accepts_owner(&key.owner) && window.current_content_matches(&key.content)
                })
                .max_by_key(|(key, revision)| (key.content.source_kind.preference(), **revision));
            if let Some((key, revision)) = best {
                window.paint_matches_current_page = true;
                window.full_texture_painted =
                    key.content.source_kind == TestScriptPaintSourceKind::FullOrProcessed;
                window.paint_source = key.content.source_kind.as_str().to_string();
                window.paint_source_texture = format!("{:?}", key.content.source_texture_id);
                window.painted_page_index = Some(key.content.page_index);
                window.paint_revision = *revision;
            }
            if let Some(identity) = &window.identity {
                window.paints = frame_paints.get(identity).cloned().unwrap_or_default();
            }
            window.sidecar_imported = sidecar_observations.contains(&(
                window.context_serial,
                window.items_generation,
                TestScriptSidecarObservation::Imported,
            ));
            window.sidecar_loaded = sidecar_observations.contains(&(
                window.context_serial,
                window.items_generation,
                TestScriptSidecarObservation::Loaded,
            ));
            window
        })
        .collect()
}

struct FinishState {
    outcome: ScriptOutcome,
    started_frame: u64,
    newer_frames: u8,
}

struct UiRuntime {
    rx: mpsc::Receiver<UiCommand>,
    snapshot: Arc<RwLock<TestScriptSnapshot>>,
    interrupt: Arc<InterruptState>,
    pending_actions: VecDeque<PendingAction>,
    smoke_actions: VecDeque<UiSmokeAction>,
    last_frame: Option<u64>,
    finish: Option<FinishState>,
    cancel_requested: bool,
    authoritative_windows: Vec<TestScriptWindowSnapshot>,
    viewport_observations: HashMap<TestScriptWindowIdentity, u64>,
    paint_observations: HashMap<TestScriptPaintEvidenceKey, u64>,
    frame_paints: HashMap<TestScriptWindowIdentity, Vec<TestScriptPaintObservation>>,
    sidecar_observations: std::collections::HashSet<(u64, u64, TestScriptSidecarObservation)>,
    next_observation_revision: u64,
    pointer_regions: pointer_input::SharedRegionCatalog,
    capture: Option<capture::Coordinator>,
    failure_capture_requested: bool,
    failure_capture_batch: Option<u64>,
}

impl UiRuntime {
    fn new(
        rx: mpsc::Receiver<UiCommand>,
        snapshot: Arc<RwLock<TestScriptSnapshot>>,
        interrupt: Arc<InterruptState>,
        pointer_regions: pointer_input::SharedRegionCatalog,
    ) -> Self {
        Self {
            rx,
            snapshot,
            interrupt,
            pending_actions: VecDeque::new(),
            smoke_actions: VecDeque::new(),
            last_frame: None,
            finish: None,
            cancel_requested: false,
            authoritative_windows: Vec::new(),
            viewport_observations: HashMap::new(),
            paint_observations: HashMap::new(),
            frame_paints: HashMap::new(),
            sidecar_observations: std::collections::HashSet::new(),
            next_observation_revision: 0,
            pointer_regions,
            capture: None,
            failure_capture_requested: false,
            failure_capture_batch: None,
        }
    }

    fn replace_authoritative_windows(&mut self, windows: Vec<TestScriptWindowSnapshot>) {
        self.authoritative_windows = windows;
        if let Ok(mut regions) = self.pointer_regions.write() {
            regions.retain_authoritative(&self.authoritative_windows);
        } else {
            self.interrupt
                .fail("test-script pointer region catalog is poisoned");
        }
        let mut retained_actions = VecDeque::with_capacity(self.pending_actions.len());
        while let Some(mut pending) = self.pending_actions.pop_front() {
            let stale_owner = match &pending.dispatch {
                PendingActionDispatch::LegacyImplicit => None,
                PendingActionDispatch::Targeted { owner, .. } => (!self
                    .authoritative_windows
                    .iter()
                    .any(|window| window.identity.as_ref() == Some(owner)))
                .then(|| owner.clone()),
            };
            if let Some(owner) = stale_owner {
                let message = format!(
                    "run_action target is no longer current: {}",
                    owner.describe()
                );
                if let Some(applied) = pending.applied.take() {
                    let _ = applied.send(Err(message.clone()));
                }
                crate::logger::log(format!(
                    "[test-script] action rejected target_mode=targeted owner={} reason=stale",
                    owner.describe()
                ));
            } else {
                retained_actions.push_back(pending);
            }
        }
        self.pending_actions = retained_actions;
        self.viewport_observations.retain(|identity, _| {
            self.authoritative_windows
                .iter()
                .any(|window| window.accepts_owner(identity))
        });
        self.paint_observations.retain(|key, _| {
            self.authoritative_windows.iter().any(|window| {
                window.accepts_owner(&key.owner) && window.current_content_matches(&key.content)
            })
        });
        self.frame_paints.retain(|identity, _| {
            self.authoritative_windows
                .iter()
                .any(|window| window.accepts_owner(identity))
        });
    }

    fn capture_targets(
        &self,
        scope: CaptureScope,
        selection: &TestScriptActionSelection,
        native_viewports: &std::collections::HashSet<egui::ViewportId>,
    ) -> Result<Vec<capture::Target>, String> {
        // eframe builds RawInput.viewports from the same native viewport table
        // that its paint dispatcher uses. An App context/host identity alone
        // can outlive that table during an active-to-passive handoff.
        let availability = |id| {
            if native_viewports.contains(&id) {
                capture::Availability::Registered
            } else {
                capture::Availability::Absent
            }
        };
        let root = capture::Target {
            viewport_id: egui::ViewportId::ROOT,
            role: "root".into(),
            hwnd: self
                .authoritative_windows
                .iter()
                .find(|window| window.role == "root")
                .and_then(|window| window.identity.as_ref())
                .map(TestScriptWindowIdentity::hwnd),
            availability: availability(egui::ViewportId::ROOT),
            presentation: TestScriptWindowPresentation::Root,
        };
        if scope == CaptureScope::Selected {
            return match selection {
                TestScriptActionSelection::LegacyImplicit => Ok(vec![root]),
                TestScriptActionSelection::Targeted(identity) => {
                    let target_window = self
                        .authoritative_windows
                        .iter()
                        .find(|window| window.identity.as_ref() == Some(identity))
                        .ok_or_else(|| {
                            format!(
                                "capture target is no longer current: {}",
                                identity.describe()
                            )
                        })?;
                    match identity {
                        TestScriptWindowIdentity::Root { .. } => Ok(vec![root]),
                        TestScriptWindowIdentity::Detached {
                            window_id,
                            context_serial,
                            viewport_id,
                            ..
                        } => Ok(vec![capture::Target {
                            viewport_id: *viewport_id,
                            role: format!("detached-{window_id}-{context_serial}"),
                            hwnd: Some(identity.hwnd()),
                            availability: availability(*viewport_id),
                            presentation: target_window.presentation,
                        }]),
                    }
                }
            };
        }
        let mut targets = vec![root];
        let mut seen = std::collections::HashSet::from([egui::ViewportId::ROOT]);
        for window in &self.authoritative_windows {
            let Some(identity) = window.identity.as_ref() else {
                continue;
            };
            if let TestScriptWindowIdentity::Detached {
                window_id,
                context_serial,
                viewport_id,
                ..
            } = identity
                && seen.insert(*viewport_id)
            {
                targets.push(capture::Target {
                    viewport_id: *viewport_id,
                    role: format!("detached-{window_id}-{context_serial}"),
                    hwnd: Some(identity.hwnd()),
                    availability: availability(*viewport_id),
                    presentation: window.presentation,
                });
            }
        }
        if let Ok(snapshot) = self.snapshot.read() {
            for style in &snapshot.host_styles {
                if style.role == "detached"
                    && let (Some(viewport_id), Some(window_id)) =
                        (style.viewport_id, style.window_id)
                    && seen.insert(viewport_id)
                {
                    targets.push(capture::Target {
                        viewport_id,
                        role: format!("detached-{window_id}"),
                        hwnd: Some(style.hwnd),
                        availability: availability(viewport_id),
                        presentation: style.presentation,
                    });
                }
                if style.role == "fullscreen"
                    && let Some(viewport_id) = style.viewport_id
                    && seen.insert(viewport_id)
                {
                    targets.push(capture::Target {
                        viewport_id,
                        role: "fullscreen".into(),
                        hwnd: Some(style.hwnd),
                        availability: availability(viewport_id),
                        presentation: TestScriptWindowPresentation::ActiveImmediate,
                    });
                }
                if style.role == "preview"
                    && let Some(viewport_id) = style.viewport_id
                    && seen.insert(viewport_id)
                {
                    targets.push(capture::Target {
                        viewport_id,
                        role: "preview".into(),
                        hwnd: Some(style.hwnd),
                        availability: availability(viewport_id),
                        presentation: TestScriptWindowPresentation::ActiveImmediate,
                    });
                }
            }
        }
        Ok(targets)
    }

    fn request_failure_capture(
        &mut self,
        ctx: &egui::Context,
        arm_watchdog: impl FnOnce(&ScriptOutcome),
    ) {
        if self.finish.is_none() || self.failure_capture_requested {
            return;
        }
        self.failure_capture_requested = true;
        let Some(outcome) = self
            .finish
            .as_ref()
            .map(|finish| &finish.outcome)
            .filter(|outcome| outcome.kind != ScriptOutcomeKind::Success)
            .cloned()
        else {
            return;
        };

        // A stalled renderer can prevent every later UI update, including the
        // update that would expire the capture batch. Guard the original result
        // before sending any screenshot command.
        arm_watchdog(&outcome);
        let native_viewports = ctx.input(|input| input.raw.viewports.keys().copied().collect());
        let targets = self.capture_targets(
            CaptureScope::All,
            &TestScriptActionSelection::LegacyImplicit,
            &native_viewports,
        );
        if let (Some(capture), Ok(targets)) = (self.capture.as_mut(), targets) {
            match capture.request(ctx, "failure", targets, capture::FAILURE_TIMEOUT, None) {
                Ok(batch) => self.failure_capture_batch = Some(batch),
                Err(error) => crate::logger::log(format!(
                    "[test-script] automatic failure screenshot unavailable: {error}"
                )),
            }
        }
    }

    fn joined_windows(&self) -> Vec<TestScriptWindowSnapshot> {
        joined_window_snapshots(
            &self.authoritative_windows,
            &self.viewport_observations,
            &self.paint_observations,
            &self.frame_paints,
            &self.sidecar_observations,
        )
    }

    fn publish_snapshot(&mut self, mut snapshot: TestScriptSnapshot) -> Result<(), String> {
        self.replace_authoritative_windows(std::mem::take(&mut snapshot.windows));
        snapshot.windows = self.joined_windows();
        self.snapshot
            .write()
            .map(|mut published| *published = snapshot)
            .map_err(|_| "test-script snapshot is poisoned".to_string())
    }

    fn publish_windows(&mut self, windows: Vec<TestScriptWindowSnapshot>) -> Result<(), String> {
        self.replace_authoritative_windows(windows);
        let joined = self.joined_windows();
        self.snapshot
            .write()
            .map(|mut published| published.windows = joined)
            .map_err(|_| "test-script snapshot is poisoned".to_string())
    }

    fn validate_selected_owner(&self, expected: &TestScriptWindowIdentity) -> Result<(), String> {
        if self.finish.is_some() {
            return Err("script is already finishing".to_string());
        }
        if self.cancel_requested {
            return Err("script input cancellation is already active".to_string());
        }
        if !self
            .authoritative_windows
            .iter()
            .any(|window| window.identity.as_ref() == Some(expected))
        {
            return Err(format!(
                "selected owner is no longer authoritative: {}",
                expected.describe()
            ));
        }
        match eframe::miv_test_script_window_witness::is_current(
            expected.viewport_id(),
            expected.hwnd(),
            expected.backend_token(),
        ) {
            Ok(true) => Ok(()),
            Ok(false) => Err(format!(
                "selected owner backend allocation is no longer current: {}",
                expected.describe()
            )),
            Err(error) => Err(format!("native window witness validation failed: {error}")),
        }
    }

    fn publish_window_frame(
        &mut self,
        owner: TestScriptWindowIdentity,
        content: Option<TestScriptContentProof>,
    ) -> Result<bool, String> {
        let drawn = take_drawn_paint(owner.viewport_id());
        let owner_is_current = self
            .authoritative_windows
            .iter()
            .any(|window| window.accepts_owner(&owner));
        if !owner_is_current {
            return Ok(false);
        }
        self.next_observation_revision = self.next_observation_revision.wrapping_add(1).max(1);
        let revision = self.next_observation_revision;
        self.viewport_observations.insert(owner.clone(), revision);
        let paints = drawn
            .into_iter()
            .filter(|paint| paint.content.context_serial == owner.context_serial())
            .map(|paint| TestScriptPaintObservation {
                owner: owner.clone(),
                content: paint.content,
                texture: paint.texture,
                vertices: paint.vertices,
                clip: paint.clip,
                viewport_size: paint.viewport_size,
                pixels_per_point: paint.pixels_per_point,
                placement: paint.placement,
                revision,
            })
            .collect();
        self.frame_paints.insert(owner.clone(), paints);
        if let Some(content) = content
            && self.authoritative_windows.iter().any(|window| {
                window.accepts_owner(&owner) && window.current_content_matches(&content)
            })
        {
            let existing_preference = self
                .paint_observations
                .iter()
                .filter(|(key, _)| key.owner == owner)
                .map(|(key, _)| key.content.source_kind.preference())
                .max();
            if existing_preference
                .is_none_or(|preference| content.source_kind.preference() >= preference)
            {
                // One current-content observation per exact owner is enough. A new
                // processed texture must not grow this table forever, while a late
                // thumbnail callback must not replace evidence that a full source was
                // already painted for the same current page.
                self.paint_observations.retain(|key, _| key.owner != owner);
                self.paint_observations
                    .insert(TestScriptPaintEvidenceKey { owner, content }, revision);
            }
        }
        let joined = self.joined_windows();
        self.snapshot
            .write()
            .map(|mut published| published.windows = joined)
            .map_err(|_| "test-script snapshot is poisoned".to_string())?;
        Ok(true)
    }

    fn begin_finish(&mut self, mut outcome: ScriptOutcome, frame: u64) {
        if let Some(existing) = self.finish.as_mut() {
            if outcome.kind == ScriptOutcomeKind::EnvironmentFailure {
                existing
                    .outcome
                    .override_with_environment_failure(outcome.message);
            }
            return;
        }
        if let Some(environment_failure) = self.interrupt.failure_message() {
            outcome = ScriptOutcome::environment_failure(environment_failure);
        }
        self.release_pending_actions("script finished before run_action was consumed");
        self.finish = Some(FinishState {
            outcome,
            started_frame: frame,
            newer_frames: 0,
        });
    }

    fn fail_environment(&mut self, message: String, frame: u64) {
        self.interrupt.fail(message.clone());
        self.release_pending_actions(&message);
        self.begin_finish(ScriptOutcome::environment_failure(message), frame);
    }

    fn request_cancel(&mut self) -> bool {
        if self.cancel_requested {
            return true;
        }
        self.cancel_requested = crate::key_input::cancel_synthetic_input(Instant::now());
        self.cancel_requested
    }

    fn release_pending_actions(&mut self, message: &str) {
        for mut pending in self.pending_actions.drain(..) {
            if let Some(applied) = pending.applied.take() {
                let _ = applied.send(Err(message.to_string()));
            }
        }
    }

    fn queue_action(
        &mut self,
        action: KeyAction,
        selection: TestScriptActionSelection,
        applied: mpsc::SyncSender<Result<(), String>>,
        target_focused: bool,
    ) -> Option<egui::ViewportId> {
        match selection {
            TestScriptActionSelection::LegacyImplicit => {
                self.pending_actions.push_back(PendingAction {
                    action,
                    dispatch: PendingActionDispatch::LegacyImplicit,
                    applied: Some(applied),
                });
                crate::logger::log(format!(
                    "[test-script] run_action action={} target_mode=legacy_implicit",
                    action.ini_name()
                ));
                None
            }
            TestScriptActionSelection::Targeted(owner) => {
                let Some(window) = self
                    .authoritative_windows
                    .iter()
                    .find(|window| window.identity.as_ref() == Some(&owner))
                else {
                    let message = format!(
                        "run_action target is no longer current: {}",
                        owner.describe()
                    );
                    let _ = applied.send(Err(message));
                    return None;
                };
                let phase = match (&owner, window.residence.as_str()) {
                    (TestScriptWindowIdentity::Root { .. }, "mounted" | "at_rest") => {
                        if target_focused {
                            TargetedActionPhase::AwaitingPass
                        } else {
                            TargetedActionPhase::AwaitingFocus
                        }
                    }
                    (TestScriptWindowIdentity::Detached { .. }, "mounted" | "at_rest") => {
                        TargetedActionPhase::AwaitingDetachedOwner
                    }
                    _ => {
                        let message = format!(
                            "run_action target cannot accept input: {} residence={}",
                            owner.describe(),
                            window.residence
                        );
                        let _ = applied.send(Err(message));
                        return None;
                    }
                };
                let focus =
                    (phase == TargetedActionPhase::AwaitingFocus).then(|| owner.viewport_id());
                crate::logger::log(format!(
                    "[test-script] run_action action={} target_mode=targeted owner={} phase={phase:?}",
                    action.ini_name(),
                    owner.describe()
                ));
                self.pending_actions.push_back(PendingAction {
                    action,
                    dispatch: PendingActionDispatch::Targeted { owner, phase },
                    applied: Some(applied),
                });
                focus
            }
        }
    }

    fn pending_targeted_detached_owner(&self) -> Option<TestScriptWindowIdentity> {
        self.pending_actions
            .iter()
            .find_map(|pending| match &pending.dispatch {
                PendingActionDispatch::Targeted {
                    owner,
                    phase: TargetedActionPhase::AwaitingDetachedOwner,
                } => Some(owner.clone()),
                PendingActionDispatch::LegacyImplicit
                | PendingActionDispatch::Targeted {
                    phase: TargetedActionPhase::AwaitingFocus | TargetedActionPhase::AwaitingPass,
                    ..
                } => None,
            })
    }

    fn finish_targeted_detached_owner(
        &mut self,
        owner: &TestScriptWindowIdentity,
        result: Result<(), String>,
    ) {
        let Some(index) = self.pending_actions.iter().position(|pending| {
            matches!(
                &pending.dispatch,
                PendingActionDispatch::Targeted {
                    owner: pending_owner,
                    phase: TargetedActionPhase::AwaitingDetachedOwner,
                } if pending_owner == owner
            )
        }) else {
            return;
        };
        match result {
            Ok(()) => {
                if let PendingActionDispatch::Targeted { phase, .. } =
                    &mut self.pending_actions[index].dispatch
                {
                    *phase = TargetedActionPhase::AwaitingFocus;
                }
                crate::logger::log(format!(
                    "[test-script] action target ready owner={}",
                    owner.describe()
                ));
            }
            Err(message) => {
                let mut pending = self.pending_actions.remove(index).expect("index exists");
                if let Some(applied) = pending.applied.take() {
                    let _ = applied.send(Err(message.clone()));
                }
                crate::logger::log(format!(
                    "[test-script] action target resolution failed owner={} error={message}",
                    owner.describe()
                ));
            }
        }
    }

    fn promote_focused_action_targets(
        &mut self,
        is_focused: impl Fn(&TestScriptWindowIdentity) -> bool,
    ) -> Vec<egui::ViewportId> {
        let mut ready = Vec::new();
        for pending in &mut self.pending_actions {
            let PendingActionDispatch::Targeted { owner, phase } = &mut pending.dispatch else {
                continue;
            };
            if *phase == TargetedActionPhase::AwaitingFocus
                && self
                    .authoritative_windows
                    .iter()
                    .any(|window| window.identity.as_ref() == Some(owner))
                && is_focused(owner)
            {
                *phase = TargetedActionPhase::AwaitingPass;
                crate::logger::log(format!(
                    "[test-script] run_action focus ready owner={}",
                    owner.describe()
                ));
                ready.push(owner.viewport_id());
            }
        }
        ready
    }

    fn expire_unconsumed_legacy_actions(&mut self, frame: u64) {
        if self.pending_actions.is_empty() {
            return;
        }
        let mut retained = VecDeque::with_capacity(self.pending_actions.len());
        let mut unconsumed = Vec::new();
        while let Some(pending) = self.pending_actions.pop_front() {
            match pending.dispatch {
                PendingActionDispatch::LegacyImplicit => {
                    if pending.applied.is_some() {
                        unconsumed.push(pending);
                    }
                }
                PendingActionDispatch::Targeted { .. } => retained.push_back(pending),
            }
        }
        self.pending_actions = retained;
        if unconsumed.is_empty() {
            return;
        }
        let names = unconsumed
            .iter()
            .map(|pending| pending.action.ini_name())
            .collect::<Vec<_>>()
            .join(", ");
        let message = format!("run_action was not consumed in its UI frame: {names}");
        for mut pending in unconsumed {
            if let Some(applied) = pending.applied.take() {
                let _ = applied.send(Err(message.clone()));
            }
        }
        self.fail_environment(message, frame);
    }

    fn finish_target_pass(&mut self, owner: &TestScriptWindowIdentity, eligible: bool, frame: u64) {
        let mut retained = VecDeque::with_capacity(self.pending_actions.len());
        let mut unconsumed = Vec::new();
        while let Some(pending) = self.pending_actions.pop_front() {
            let belongs_to_pass = matches!(
                &pending.dispatch,
                PendingActionDispatch::Targeted {
                    owner: pending_owner,
                    phase: TargetedActionPhase::AwaitingPass,
                } if pending_owner == owner
            );
            if belongs_to_pass && (eligible || pending.applied.is_none()) {
                if eligible && pending.applied.is_some() {
                    unconsumed.push(pending);
                }
            } else {
                retained.push_back(pending);
            }
        }
        self.pending_actions = retained;
        if unconsumed.is_empty() {
            return;
        }
        let names = unconsumed
            .iter()
            .map(|pending| pending.action.ini_name())
            .collect::<Vec<_>>()
            .join(", ");
        let message = format!(
            "run_action was not consumed in its target UI pass: owner={} actions={names}",
            owner.describe()
        );
        for mut pending in unconsumed {
            if let Some(applied) = pending.applied.take() {
                let _ = applied.send(Err(message.clone()));
            }
        }
        self.fail_environment(message, frame);
    }
}

fn runtime() -> &'static Mutex<Option<UiRuntime>> {
    static RUNTIME: OnceLock<Mutex<Option<UiRuntime>>> = OnceLock::new();
    RUNTIME.get_or_init(|| Mutex::new(None))
}

static PROCESS_EXIT_CODE: AtomicI32 = AtomicI32::new(EXIT_NOT_SET);
static SHUTDOWN_WATCHDOG_ARMED: AtomicBool = AtomicBool::new(false);

fn frame_key(ctx: &egui::Context) -> u64 {
    ctx.input(|input| input.time.to_bits())
}

fn describe_issue(issue: &SyntheticInputIssue) -> String {
    match issue {
        SyntheticInputIssue::WaitingForRouting(error) => {
            format!("synthetic routing target unavailable: {error:?}")
        }
        SyntheticInputIssue::WaitingForFocus(target) => format!(
            "synthetic routing target is not focused: viewport={:?} hwnd=0x{:x}",
            target.viewport, target.hwnd
        ),
        SyntheticInputIssue::FocusLost { viewport } => {
            format!("synthetic key hold lost focus: viewport={viewport:?}")
        }
        SyntheticInputIssue::TargetViewportNotRendered {
            viewport,
            raw_input_time,
            event_count,
        } => format!(
            "synthetic target viewport was not rendered in its outer frame: viewport={viewport:?} raw_input_time={raw_input_time:?} event_count={event_count}"
        ),
        SyntheticInputIssue::PointerOwnerMismatch { handle, detail } => {
            format!(
                "synthetic pointer owner mismatch: step={} detail={detail}",
                handle.step_id
            )
        }
        SyntheticInputIssue::PointerPhysicalInputMixed { handle } => {
            format!(
                "physical pointer input mixed with synthetic transaction: step={}",
                handle.step_id
            )
        }
        SyntheticInputIssue::PointerViewportNotRendered { handle, viewport } => format!(
            "synthetic pointer viewport was not rendered: step={} viewport={viewport:?}",
            handle.step_id
        ),
        SyntheticInputIssue::MissingPointerShowTail { handle, viewport } => format!(
            "synthetic pointer show ended without callback tail: step={} viewport={viewport:?}",
            handle.step_id
        ),
    }
}

pub(crate) fn start(path: PathBuf, run_dir: PathBuf, ctx: &egui::Context) -> Result<(), String> {
    PROCESS_EXIT_CODE.store(EXIT_NOT_SET, Ordering::Release);
    let result = start_inner(path, run_dir, ctx);
    if let Err(error) = &result {
        let outcome = ScriptOutcome::environment_failure(format!("runner start failed: {error}"));
        PROCESS_EXIT_CODE.store(EXIT_ENVIRONMENT_FAILURE, Ordering::Release);
        crate::logger::log(format!(
            "[test-script] finished kind=EnvironmentFailure exit_code={} message=runner start failed: {error}",
            EXIT_ENVIRONMENT_FAILURE
        ));
        emit_perf_fail(&outcome);
        let _ = crate::key_input::cancel_synthetic_input(Instant::now());
        crate::key_input::disarm_synthetic_input();
        // eframe has already created its wgpu Device before invoking the app
        // creator, so even creator failure needs the teardown watchdog.
        arm_shutdown_watchdog(EXIT_ENVIRONMENT_FAILURE, "runner-start-failure");
    }
    result
}

fn start_inner(path: PathBuf, run_dir: PathBuf, ctx: &egui::Context) -> Result<(), String> {
    let capture = capture::Coordinator::new(&run_dir)?;
    if !crate::key_input::arm_synthetic_input() {
        return Err("failed to arm synthetic input timeline".to_string());
    }
    // Synthetic routing intentionally obeys the production foreground/focus
    // rules. Request focus here so unattended runs can satisfy that precondition
    // without a host-side click or key injection.
    ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);

    let (tx, rx) = mpsc::channel();
    let snapshot = Arc::new(RwLock::new(TestScriptSnapshot::default()));
    let interrupt = Arc::new(InterruptState::default());
    let pointer_regions = Arc::new(RwLock::new(pointer_input::RegionCatalog::default()));
    let wake_ctx = ctx.clone();
    let bridge = RunnerBridge {
        tx,
        snapshot: Arc::clone(&snapshot),
        interrupt: Arc::clone(&interrupt),
        wake: Arc::new(move || {
            wake_ctx.request_repaint_of(egui::ViewportId::ROOT);
        }),
        next_hold_id: Arc::new(AtomicU64::new(0)),
        action_selection: Arc::new(Mutex::new(TestScriptActionSelection::LegacyImplicit)),
        pointer_regions: Arc::clone(&pointer_regions),
    };

    let mut guard = runtime()
        .lock()
        .map_err(|_| "test-script runtime is poisoned".to_string())?;
    if guard.is_some() {
        crate::key_input::disarm_synthetic_input();
        return Err("a test-script runtime is already active".to_string());
    }
    let mut active = UiRuntime::new(rx, snapshot, interrupt, pointer_regions);
    active.capture = Some(capture);
    *guard = Some(active);
    drop(guard);

    if let Err(error) = spawn_script_path(path, bridge) {
        if let Ok(mut guard) = runtime().lock() {
            *guard = None;
        }
        crate::key_input::disarm_synthetic_input();
        return Err(error);
    }
    ctx.request_repaint_of(egui::ViewportId::ROOT);
    Ok(())
}

pub(crate) fn action_target_is_focused(
    ctx: &egui::Context,
    owner: &TestScriptWindowIdentity,
) -> bool {
    if ctx.input_for(owner.viewport_id(), |input| input.viewport().focused) != Some(true) {
        return false;
    }
    if !eframe::miv_test_script_window_witness::is_current(
        owner.viewport_id(),
        owner.hwnd(),
        owner.backend_token(),
    )
    .unwrap_or(false)
    {
        return false;
    }
    #[cfg(windows)]
    {
        use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
        unsafe { GetForegroundWindow().0 as usize as u64 == owner.hwnd() }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn action_matches_owner(
    dispatch: &PendingActionDispatch,
    owner: Option<&TestScriptWindowIdentity>,
) -> bool {
    match dispatch {
        PendingActionDispatch::LegacyImplicit => true,
        PendingActionDispatch::Targeted {
            owner: target,
            phase: TargetedActionPhase::AwaitingPass,
        } => owner == Some(target),
        PendingActionDispatch::Targeted {
            phase: TargetedActionPhase::AwaitingDetachedOwner | TargetedActionPhase::AwaitingFocus,
            ..
        } => false,
    }
}

fn consume_pending_action_from(
    pending_actions: &mut VecDeque<PendingAction>,
    owner: Option<&TestScriptWindowIdentity>,
    action: KeyAction,
) -> bool {
    let Some(index) = pending_actions.iter().position(|pending| {
        pending.action == action && action_matches_owner(&pending.dispatch, owner)
    }) else {
        return false;
    };
    let mut pending = pending_actions.remove(index).expect("index exists");
    if let Some(applied) = pending.applied.take() {
        let _ = applied.send(Ok(()));
    }
    true
}

fn peek_pending_action_from(
    pending_actions: &mut VecDeque<PendingAction>,
    owner: Option<&TestScriptWindowIdentity>,
    action: KeyAction,
) -> bool {
    let Some(pending) = pending_actions
        .iter_mut()
        .find(|pending| pending.action == action && action_matches_owner(&pending.dispatch, owner))
    else {
        return false;
    };
    if let Some(applied) = pending.applied.take() {
        let _ = applied.send(Ok(()));
    }
    true
}

fn action_pass_observation_id(viewport_id: egui::ViewportId) -> egui::Id {
    egui::Id::new("miv.test_script.action_pass_observation").with(viewport_id)
}

fn action_pass_observation(ctx: &egui::Context) -> Option<TestScriptActionPassObservation> {
    let pass = ctx.cumulative_pass_nr();
    let observation_id = action_pass_observation_id(ctx.viewport_id());
    ctx.data(|data| {
        data.get_temp::<TestScriptActionPassObservation>(observation_id)
            .filter(|observation| observation.pass == pass)
    })
}

fn action_pass_observation_for_active_backend(
    ctx: &egui::Context,
) -> Option<TestScriptActionPassObservation> {
    let observation = action_pass_observation(ctx)?;
    if observation.owner.as_ref().is_none_or(|owner| {
        eframe::miv_test_script_window_witness::active()
            .is_some_and(|witness| owner.matches_backend_witness(witness))
    }) {
        Some(observation)
    } else {
        None
    }
}

pub(crate) fn publish_action_pass_owner(
    ctx: &egui::Context,
    owner: Option<TestScriptWindowIdentity>,
) {
    let owner = owner.filter(|owner| {
        eframe::miv_test_script_window_witness::active()
            .is_some_and(|witness| owner.matches_backend_witness(witness))
    });
    let pass = ctx.cumulative_pass_nr();
    let observation_id = action_pass_observation_id(ctx.viewport_id());
    ctx.data_mut(|data| {
        let eligible = data
            .get_temp::<TestScriptActionPassObservation>(observation_id)
            .is_some_and(|observation| {
                observation.pass == pass && observation.owner == owner && observation.eligible
            });
        data.insert_temp(
            observation_id,
            TestScriptActionPassObservation {
                pass,
                owner,
                eligible,
            },
        );
    });
}

pub(crate) fn mark_action_pass_eligible(ctx: &egui::Context) {
    let Some(mut observation) = action_pass_observation_for_active_backend(ctx) else {
        return;
    };
    observation.eligible = true;
    let observation_id = action_pass_observation_id(ctx.viewport_id());
    ctx.data_mut(|data| data.insert_temp(observation_id, observation));
}

pub(crate) fn finish_action_pass(ctx: &egui::Context) {
    let Some(observation) = action_pass_observation_for_active_backend(ctx) else {
        return;
    };
    let Some(owner) = observation.owner else {
        return;
    };
    let Ok(mut guard) = runtime().lock() else {
        return;
    };
    let Some(runtime) = guard.as_mut() else {
        return;
    };
    runtime.finish_target_pass(&owner, observation.eligible, frame_key(ctx));
}

pub(crate) fn consume_pending_action(ctx: &egui::Context, action: KeyAction) -> bool {
    let owner =
        action_pass_observation_for_active_backend(ctx).and_then(|observation| observation.owner);
    let Ok(mut guard) = runtime().lock() else {
        return false;
    };
    let Some(runtime) = guard.as_mut() else {
        return false;
    };
    consume_pending_action_from(&mut runtime.pending_actions, owner.as_ref(), action)
}

pub(crate) fn peek_pending_action(ctx: &egui::Context, action: KeyAction) -> bool {
    let owner =
        action_pass_observation_for_active_backend(ctx).and_then(|observation| observation.owner);
    let Ok(mut guard) = runtime().lock() else {
        return false;
    };
    let Some(runtime) = guard.as_mut() else {
        return false;
    };
    peek_pending_action_from(&mut runtime.pending_actions, owner.as_ref(), action)
}

pub(crate) fn pending_targeted_detached_owner() -> Option<TestScriptWindowIdentity> {
    runtime()
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref()?.pending_targeted_detached_owner())
}

pub(crate) fn finish_targeted_detached_owner(
    owner: &TestScriptWindowIdentity,
    result: Result<(), String>,
) {
    let Ok(mut guard) = runtime().lock() else {
        return;
    };
    let Some(runtime) = guard.as_mut() else {
        return;
    };
    runtime.finish_targeted_detached_owner(owner, result);
}

fn flush_exit_logs() {
    crate::perf::flush();
    crate::logger::flush();
}

fn arm_shutdown_watchdog(exit_code: i32, trigger: &'static str) {
    if SHUTDOWN_WATCHDOG_ARMED.swap(true, Ordering::AcqRel) {
        return;
    }
    crate::logger::log(format!(
        "[test-script] shutdown path=watchdog-armed trigger={trigger} grace_ms={}",
        SHUTDOWN_WATCHDOG_GRACE.as_millis()
    ));
    // Persist every event produced before Close even if third-party GPU
    // teardown wedges before the watchdog reaches its final flush.
    flush_exit_logs();

    let spawned = std::thread::Builder::new()
        .name("test-script-exit-watchdog".to_string())
        .spawn(move || {
            std::thread::sleep(SHUTDOWN_WATCHDOG_GRACE);
            // This is deliberately process-level containment, not a repair for
            // wgpu. Once eframe owns Device teardown there is no application
            // API with which to complete or cancel its stuck GPU submission.
            // The opt-in test harness must nevertheless expose the already
            // determined script result as the real process exit code.
            crate::logger::log(format!(
                "[test-script] shutdown path=forced-watchdog exit_code={exit_code}"
            ));
            flush_exit_logs();
            std::process::exit(exit_code);
        });
    if let Err(error) = spawned {
        crate::logger::log(format!(
            "[test-script] shutdown path=watchdog-spawn-failed exit_code={} error={error}",
            EXIT_ENVIRONMENT_FAILURE
        ));
        flush_exit_logs();
        std::process::exit(EXIT_ENVIRONMENT_FAILURE);
    }
}

pub(crate) fn receive_screenshot_events(ctx: &egui::Context, kind: &'static str) {
    let Ok(mut guard) = runtime().lock() else {
        return;
    };
    if let Some(capture) = guard.as_mut().and_then(|runtime| runtime.capture.as_mut()) {
        capture.note_pass(ctx.viewport_id(), kind);
        capture.receive_events(ctx);
        capture.poll();
    }
}

pub(crate) fn capture_pending_for(viewport_id: egui::ViewportId) -> bool {
    runtime()
        .lock()
        .ok()
        .and_then(|guard| {
            guard
                .as_ref()
                .and_then(|runtime| runtime.capture.as_ref())
                .map(|capture| capture.pending_viewport(viewport_id))
        })
        .unwrap_or(false)
}

pub(crate) fn ui_update(ctx: &egui::Context, snapshot: TestScriptSnapshot) -> bool {
    let frame = frame_key(ctx);
    let issues = crate::key_input::take_synthetic_input_issues(ctx);
    let Ok(mut guard) = runtime().lock() else {
        let outcome =
            ScriptOutcome::environment_failure("test-script runtime is poisoned".to_string());
        PROCESS_EXIT_CODE.store(EXIT_ENVIRONMENT_FAILURE, Ordering::Release);
        crate::logger::log(format!(
            "[test-script] finished kind=EnvironmentFailure exit_code={} message=test-script runtime is poisoned",
            EXIT_ENVIRONMENT_FAILURE
        ));
        emit_perf_fail(&outcome);
        let _ = crate::key_input::cancel_synthetic_input(Instant::now());
        crate::key_input::disarm_synthetic_input();
        arm_shutdown_watchdog(EXIT_ENVIRONMENT_FAILURE, "runtime-poisoned");
        return true;
    };
    let Some(runtime) = guard.as_mut() else {
        return false;
    };
    if let Some(capture) = runtime.capture.as_mut() {
        capture.receive_events(ctx);
        capture.poll();
    }

    let new_frame = runtime.last_frame != Some(frame);
    if new_frame {
        if runtime.last_frame.is_some() {
            runtime.expire_unconsumed_legacy_actions(frame);
        }
        runtime.last_frame = Some(frame);
    }
    emit_perf_level_reads(&snapshot.keymap_level_observations);
    if let Err(error) = runtime.publish_snapshot(snapshot) {
        runtime.fail_environment(error, frame);
    }
    // A Focus command queued by RunAction reaches the backend only after this
    // root pass. Promote requests from earlier passes before draining new ones.
    for viewport_id in
        runtime.promote_focused_action_targets(|owner| action_target_is_focused(ctx, owner))
    {
        ctx.request_repaint_of(viewport_id);
    }
    if let Some(capture) = runtime.capture.as_mut() {
        capture.begin_root_frame(&runtime.authoritative_windows);
    }

    for issue in issues {
        let terminal_pointer_step = match &issue {
            SyntheticInputIssue::PointerOwnerMismatch { handle, .. }
            | SyntheticInputIssue::PointerPhysicalInputMixed { handle }
            | SyntheticInputIssue::PointerViewportNotRendered { handle, .. }
            | SyntheticInputIssue::MissingPointerShowTail { handle, .. } => Some(handle.clone()),
            _ => None,
        };
        runtime.fail_environment(describe_issue(&issue), frame);
        if let Some(handle) = terminal_pointer_step {
            // EnvironmentFailure is committed before the typed terminal pointer phase releases
            // to Idle, so a nominal Finished in this same frame cannot mask missing cleanup proof.
            let _ = crate::key_input::acknowledge_synthetic_pointer_terminal_issue(&handle);
        }
    }

    while let Ok(command) = runtime.rx.try_recv() {
        match command {
            UiCommand::Key(command) => {
                if runtime.finish.is_none() && !crate::key_input::enqueue_synthetic_command(command)
                {
                    runtime.fail_environment(
                        "failed to enqueue synthetic key command".to_string(),
                        frame,
                    );
                }
            }
            UiCommand::Cancel(at) => {
                runtime.release_pending_actions("run_action was cancelled before consumption");
                if crate::key_input::cancel_synthetic_input(at) {
                    runtime.cancel_requested = true;
                } else {
                    runtime.fail_environment(
                        "failed to enqueue synthetic CancelAll".to_string(),
                        frame,
                    );
                }
            }
            UiCommand::SetRepeat { delay, hz } => {
                if runtime.finish.is_none() && !crate::key_input::set_synthetic_repeat(delay, hz) {
                    runtime.fail_environment(
                        "failed to apply synthetic repeat settings".to_string(),
                        frame,
                    );
                }
            }
            UiCommand::RunAction {
                action,
                selection,
                applied,
            } => {
                if runtime.finish.is_some() {
                    let _ = applied.send(Err("script is already finishing".to_string()));
                } else {
                    let target_focused = match &selection {
                        TestScriptActionSelection::Targeted(owner) => {
                            action_target_is_focused(ctx, owner)
                        }
                        TestScriptActionSelection::LegacyImplicit => true,
                    };
                    if let Some(viewport_id) =
                        runtime.queue_action(action, selection, applied, target_focused)
                    {
                        ctx.send_viewport_cmd_to(viewport_id, egui::ViewportCommand::Focus);
                        ctx.request_repaint_of(viewport_id);
                    }
                }
            }
            UiCommand::SmokeAction(action) => {
                if runtime.finish.is_none() {
                    runtime.smoke_actions.push_back(action);
                }
            }
            UiCommand::ClickWidget { label, reply } => {
                if runtime.finish.is_some() {
                    let _ = reply.send(Err("script is already finishing".to_string()));
                } else if let Err(message) = request_widget_click(label, reply.clone()) {
                    let _ = reply.send(Err(message));
                } else {
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                }
            }
            UiCommand::SortPopupPointer { label, kind, reply } => {
                if runtime.finish.is_some() {
                    let _ = reply.send(Err("script is already finishing".to_string()));
                } else if let Err(message) = request_widget_pointer(
                    label,
                    WidgetPointerRequestKind::SortPopup(kind),
                    reply.clone(),
                ) {
                    let _ = reply.send(Err(message));
                } else {
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                }
            }
            UiCommand::DetailsPreviewPointer { reply } => {
                if runtime.finish.is_some() {
                    let _ = reply.send(Err("script is already finishing".to_string()));
                } else {
                    WIDGET_CLICK_DRIVER.with(|driver| {
                        let mut driver = driver.borrow_mut();
                        if driver.details_preview_move.is_some() {
                            let _ =
                                reply
                                    .send(Err("another Details preview pointer move is pending"
                                        .to_string()));
                        } else {
                            driver.details_preview_move = Some(reply);
                        }
                    });
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                }
            }
            UiCommand::ValidateSelectedOwner {
                expected_identity,
                reply,
            } => {
                let result = runtime.validate_selected_owner(&expected_identity);
                if let Err(message) = result.as_ref() {
                    runtime.fail_environment(message.clone(), frame);
                }
                let _ = reply.send(result);
            }
            UiCommand::Capture {
                label,
                scope,
                selection,
                reply,
            } => {
                let native_viewports =
                    ctx.input(|input| input.raw.viewports.keys().copied().collect());
                let result = if runtime.finish.is_some() {
                    Err("script is already finishing".to_string())
                } else {
                    runtime
                        .capture_targets(scope, &selection, &native_viewports)
                        .and_then(|targets| {
                            for target in targets.iter().take(16) {
                                let window = runtime.authoritative_windows.iter().find(|window| window.viewport_id == target.viewport_id);
                                crate::logger::log(format!(
                                    "[capture-probe] batch_start label={label} role={} viewport={:?} expected={} residence={} presentation={:?} backend_token={:?} hwnd={:?} native_registered={}",
                                    target.role,
                                    target.viewport_id,
                                    window.and_then(|window| window.identity.as_ref()).map(|identity| identity.describe()).unwrap_or_else(|| "missing".into()),
                                    window.map(|window| window.residence.as_str()).unwrap_or("missing"),
                                    target.presentation,
                                    window.and_then(|window| window.backend_token),
                                    window.and_then(|window| window.hwnd),
                                    native_viewports.contains(&target.viewport_id)
                                ));
                            }
                            if targets.len() > 16 {
                                crate::logger::log(format!(
                                    "[capture-probe] batch_start additional_targets_suppressed={}",
                                    targets.len() - 16
                                ));
                            }
                            runtime
                                .capture
                                .as_mut()
                                .ok_or_else(|| {
                                    "screenshot evidence output is unavailable".to_string()
                                })?
                                .request(
                                    ctx,
                                    &label,
                                    targets,
                                    capture::EXPLICIT_TIMEOUT,
                                    Some(reply.clone()),
                                )
                                .map(|_| ())
                        })
                };
                if let Err(error) = result {
                    let _ = reply.send(Err(error));
                }
            }
            UiCommand::Log(message) => {
                crate::logger::log(format!("[test-script] {message}"));
                emit_perf_step(&message);
            }
            UiCommand::Precondition(trace) => emit_perf_precondition(&trace),
            UiCommand::Finished(outcome) => runtime.begin_finish(outcome, frame),
        }
    }

    if runtime.finish.is_some() && !runtime.request_cancel() {
        runtime.fail_environment(
            "failed to request terminal synthetic cancellation".to_string(),
            frame,
        );
    }

    runtime.request_failure_capture(ctx, |outcome| {
        let exit_code = outcome.kind.exit_code();
        PROCESS_EXIT_CODE.store(exit_code, Ordering::Release);
        crate::logger::log(format!(
            "[test-script] failure capture guarded kind={:?} exit_code={exit_code} message={}",
            outcome.kind, outcome.message
        ));
        arm_shutdown_watchdog(exit_code, "failure-capture");
    });
    if let Some(capture) = runtime.capture.as_mut() {
        capture.poll();
    }

    let mut close = false;
    if let Some(finish) = runtime.finish.as_mut() {
        if new_frame && frame != finish.started_frame {
            finish.newer_frames = finish.newer_frames.saturating_add(1);
        }
        let capture_done = runtime.failure_capture_batch.is_none_or(|batch| {
            runtime
                .capture
                .as_ref()
                .is_none_or(|capture| !capture.is_pending(batch))
        });
        if finish.newer_frames >= 2 && crate::key_input::synthetic_input_is_idle() && capture_done {
            let outcome = finish.outcome.clone();
            PROCESS_EXIT_CODE.store(outcome.kind.exit_code(), Ordering::Release);
            crate::logger::log(format!(
                "[test-script] finished kind={:?} exit_code={} message={}",
                outcome.kind,
                outcome.kind.exit_code(),
                outcome.message
            ));
            emit_perf_fail(&outcome);
            crate::key_input::disarm_synthetic_input();
            arm_shutdown_watchdog(outcome.kind.exit_code(), "close-request");
            close = true;
        } else {
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    } else if !crate::key_input::synthetic_input_is_idle() || widget_click_in_progress() {
        // A held key must keep advancing the deterministic repeat timeline even
        // while the application would otherwise sleep.
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    if close {
        *guard = None;
    }
    close
}

/// Replace the read-only detached-window table after a lifecycle phase that can
/// establish or retire an HWND claim. This never drives the lifecycle itself.
pub(crate) fn publish_window_snapshots(windows: Vec<TestScriptWindowSnapshot>) {
    let Ok(mut guard) = runtime().lock() else {
        return;
    };
    let Some(runtime) = guard.as_mut() else {
        return;
    };
    let _ = runtime.publish_windows(windows);
}

/// Record one viewport callback and, when supplied, the exact texture command
/// queued by that callback. The current table is checked before either record is
/// exposed, so a callback from a retired host cannot displace current evidence.
pub(crate) fn publish_window_frame(
    owner: TestScriptWindowIdentity,
    content: Option<TestScriptContentProof>,
) {
    let viewport = owner.viewport_id();
    let Ok(mut guard) = runtime().lock() else {
        discard_drawn_paint(viewport);
        return;
    };
    let Some(runtime) = guard.as_mut() else {
        discard_drawn_paint(viewport);
        return;
    };
    let _ = runtime.publish_window_frame(owner, content);
}

/// The restore worker applied at least one sidecar value to the central store.
pub(crate) fn publish_sidecar_import(context_serial: u64, items_generation: u64) {
    publish_sidecar_observation(
        context_serial,
        items_generation,
        TestScriptSidecarObservation::Imported,
    );
}

/// A nonempty, validated sidecar reached the terminal restore path for this context.
pub(crate) fn publish_sidecar_load(context_serial: u64, items_generation: u64) {
    publish_sidecar_observation(
        context_serial,
        items_generation,
        TestScriptSidecarObservation::Loaded,
    );
}

fn publish_sidecar_observation(
    context_serial: u64,
    items_generation: u64,
    observation: TestScriptSidecarObservation,
) {
    let Ok(mut guard) = runtime().lock() else {
        return;
    };
    let Some(runtime) = guard.as_mut() else {
        return;
    };
    runtime
        .sidecar_observations
        .insert((context_serial, items_generation, observation));
    let joined = runtime.joined_windows();
    if let Ok(mut snapshot) = runtime.snapshot.write() {
        snapshot.windows = joined;
    }
}

pub(crate) fn publish_pointer_show(
    ctx: &egui::Context,
    output: pointer_input::ShowOutput,
    current_identity: TestScriptWindowIdentity,
    current_items_generation: u64,
    page_after_navigation: usize,
) {
    let frame_key = frame_key(ctx);
    let delivered_handle = output.delivered_cancel_handle();
    let pointer_owner = output.pointer_owner();
    let catalog_owner = output.catalog_owner();
    let has_pointer_obligation = output.has_pointer_obligation();
    let result = pointer_input::join_show_output(
        output,
        &current_identity,
        current_items_generation,
        page_after_navigation,
    );
    let Ok(mut guard) = runtime().lock() else {
        return;
    };
    let Some(runtime) = guard.as_mut() else {
        return;
    };
    match result {
        Ok(joined) => {
            let published_revision = runtime
                .pointer_regions
                .write()
                .map(|mut catalog| catalog.publish(joined.frame.clone()))
                // A PoisonError owns the failed guard. Erase it before mutably borrowing runtime
                // for the environment failure path below.
                .map_err(|_| ());
            let published_revision = match published_revision {
                Ok(revision) => revision,
                Err(_) => {
                    let error = "test-script pointer region catalog is poisoned".to_string();
                    runtime.fail_environment(error.clone(), frame_key);
                    let terminal_handle = if let Some(handle) = delivered_handle {
                        crate::key_input::fail_synthetic_pointer_delivered(&handle, error)
                            .then_some(handle)
                    } else {
                        pointer_owner.as_ref().and_then(|owner| {
                            crate::key_input::fail_synthetic_pointer_owner_lost(owner)
                        })
                    };
                    if let Some(handle) = terminal_handle {
                        let _ =
                            crate::key_input::acknowledge_synthetic_pointer_terminal_issue(&handle);
                    }
                    return;
                }
            };
            // Drop the catalog write guard before the timeline can wake the Rhai worker. Its
            // completion now carries this exact published revision as the next-paint barrier.
            if let Err(error) = joined.finish(published_revision) {
                let catalog_invalidated = runtime
                    .pointer_regions
                    .write()
                    .map(|mut catalog| catalog.invalidate(catalog_owner.as_ref()))
                    .is_ok();
                if !catalog_invalidated {
                    runtime.fail_environment(
                        "test-script pointer region catalog is poisoned".to_string(),
                        frame_key,
                    );
                }
                if !has_pointer_obligation {
                    return;
                }
                runtime.fail_environment(error.clone(), frame_key);
                let terminal_handle = if let Some(handle) = delivered_handle {
                    crate::key_input::fail_synthetic_pointer_delivered(&handle, error)
                        .then_some(handle)
                } else {
                    pointer_owner.as_ref().and_then(|owner| {
                        crate::key_input::fail_synthetic_pointer_owner_lost(owner)
                    })
                };
                if let Some(handle) = terminal_handle {
                    let _ = crate::key_input::acknowledge_synthetic_pointer_terminal_issue(&handle);
                }
            }
        }
        Err(error) => {
            let catalog_invalidated = runtime
                .pointer_regions
                .write()
                .map(|mut catalog| catalog.invalidate(catalog_owner.as_ref()))
                .is_ok();
            if !catalog_invalidated {
                runtime.fail_environment(
                    "test-script pointer region catalog is poisoned".to_string(),
                    frame_key,
                );
            }
            if !has_pointer_obligation {
                return;
            }
            runtime.fail_environment(error.clone(), frame_key);
            let terminal_handle = if let Some(handle) = delivered_handle {
                crate::key_input::fail_synthetic_pointer_delivered(&handle, error).then_some(handle)
            } else {
                pointer_owner
                    .as_ref()
                    .and_then(|owner| crate::key_input::fail_synthetic_pointer_owner_lost(owner))
            };
            if let Some(handle) = terminal_handle {
                // This direct post-show error has already reached fail_environment. It can now
                // release the typed terminal phase without waiting for issue polling.
                let _ = crate::key_input::acknowledge_synthetic_pointer_terminal_issue(&handle);
            }
        }
    }
}

pub(crate) fn reject_pointer_show(
    ctx: &egui::Context,
    output: pointer_input::ShowOutput,
    error: String,
) {
    let delivered_handle = output.delivered_cancel_handle();
    let pointer_owner = output.pointer_owner();
    let catalog_owner = output.catalog_owner();
    let has_pointer_obligation = output.has_pointer_obligation();
    let Ok(mut guard) = runtime().lock() else {
        return;
    };
    let Some(runtime) = guard.as_mut() else {
        return;
    };
    let catalog_invalidated = runtime
        .pointer_regions
        .write()
        .map(|mut catalog| catalog.invalidate(catalog_owner.as_ref()))
        .is_ok();
    if !catalog_invalidated {
        runtime.fail_environment(
            "test-script pointer region catalog is poisoned".to_string(),
            frame_key(ctx),
        );
    }
    if !has_pointer_obligation {
        return;
    }
    runtime.fail_environment(error.clone(), frame_key(ctx));
    let terminal_handle = if let Some(handle) = delivered_handle {
        crate::key_input::fail_synthetic_pointer_delivered(&handle, error).then_some(handle)
    } else {
        pointer_owner
            .as_ref()
            .and_then(crate::key_input::fail_synthetic_pointer_owner_lost)
    };
    if let Some(handle) = terminal_handle {
        let _ = crate::key_input::acknowledge_synthetic_pointer_terminal_issue(&handle);
    }
}

pub(crate) fn publish_fullscreen_input_state(
    ctx: &egui::Context,
    fs_idx: usize,
    focused: bool,
    target_rendered: bool,
    raw_key_permit: bool,
    ime_active: bool,
    text_input_or_pending_focus: bool,
    popup_open: bool,
) {
    let Ok(guard) = runtime().lock() else {
        return;
    };
    let Some(runtime) = guard.as_ref() else {
        return;
    };
    let Ok(mut snapshot) = runtime.snapshot.write() else {
        return;
    };
    if snapshot.fs_idx != fs_idx as i64 {
        return;
    }
    let viewport_label = if ctx.viewport_id() == egui::ViewportId::ROOT {
        "ROOT".to_string()
    } else {
        format!("{:?}", ctx.viewport_id())
    };
    if snapshot.target_viewport != viewport_label {
        return;
    }
    snapshot.focused = focused;
    snapshot.target_rendered = target_rendered;
    snapshot.fullscreen_raw_key_permit = raw_key_permit;
    snapshot.ime_active = ime_active;
    snapshot.text_input_or_pending_focus = text_input_or_pending_focus;
    snapshot.popup_open = popup_open;
}

pub(crate) fn on_app_exit() {
    let active = runtime().lock().ok().and_then(|mut guard| guard.take());
    let Some(mut runtime) = active else {
        return;
    };
    PROCESS_EXIT_CODE.store(EXIT_ENVIRONMENT_FAILURE, Ordering::Release);
    let outcome = ScriptOutcome::environment_failure(
        "application exited before the test script completed".to_string(),
    );
    runtime
        .interrupt
        .fail("application exited before the test script completed");
    runtime.release_pending_actions("application exited before run_action was consumed");
    let _ = crate::key_input::cancel_synthetic_input(Instant::now());
    crate::key_input::disarm_synthetic_input();
    crate::logger::log(format!(
        "[test-script] finished kind=EnvironmentFailure exit_code={} message=application exited before the test script completed",
        EXIT_ENVIRONMENT_FAILURE
    ));
    emit_perf_fail(&outcome);
    arm_shutdown_watchdog(EXIT_ENVIRONMENT_FAILURE, "premature-app-exit");
}

pub(crate) fn process_exit_code() -> Option<i32> {
    let code = PROCESS_EXIT_CODE.load(Ordering::Acquire);
    (code != EXIT_NOT_SET).then_some(code)
}

pub(crate) fn exit_after_run_native() -> ! {
    let exit_code = process_exit_code().unwrap_or(EXIT_ENVIRONMENT_FAILURE);
    crate::logger::log(format!(
        "[test-script] shutdown path=run-native-return exit_code={exit_code}"
    ));
    flush_exit_logs();
    std::process::exit(exit_code);
}

pub(crate) fn cli_script_path_from(args: &[std::ffi::OsString]) -> Result<Option<PathBuf>, String> {
    let mut script_path = None;
    let mut has_data_dir = false;
    let mut index = 1usize;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            break;
        }
        if arg == "--test-script" || arg == "--data-dir" {
            let flag = arg.to_string_lossy();
            let Some(value) = args.get(index + 1) else {
                return Err(format!("{flag} requires a path value"));
            };
            if value.to_str().is_some_and(|value| value.starts_with("--")) {
                return Err(format!("{flag} requires a path value"));
            }
            if value.is_empty() {
                return Err(format!("{flag} requires a non-empty path value"));
            }
            if arg == "--test-script" {
                if script_path.is_some() {
                    return Err("--test-script may only be specified once".to_string());
                }
                script_path = Some(PathBuf::from(value));
            } else {
                has_data_dir = true;
            }
            index += 2;
            continue;
        }
        index += 1;
    }

    if script_path.is_some() && !has_data_dir {
        return Err(
            "--test-script requires an explicit --data-dir; refusing to use the normal application profile"
                .to_string(),
        );
    }
    Ok(script_path)
}

pub(crate) fn cli_capture_dir_from(args: &[std::ffi::OsString]) -> Result<Option<PathBuf>, String> {
    let mut run_dir = None;
    let mut index = 1usize;
    while index < args.len() {
        if args[index] == "--" {
            break;
        }
        if args[index] == "--test-evidence-dir" {
            if run_dir.is_some() {
                return Err("--test-evidence-dir may only be specified once".into());
            }
            let value = args
                .get(index + 1)
                .ok_or("--test-evidence-dir requires a path value")?;
            if value.is_empty() || value.to_string_lossy().starts_with("--") {
                return Err("--test-evidence-dir requires a path value".into());
            }
            run_dir = Some(PathBuf::from(value));
            index += 2;
        } else {
            index += 1;
        }
    }
    let scripted = cli_script_path_from(args)?.is_some();
    if scripted && run_dir.is_none() {
        return Err("--test-script requires --test-evidence-dir".into());
    }
    if !scripted && run_dir.is_some() {
        return Err("--test-evidence-dir requires --test-script".into());
    }
    Ok(run_dir)
}

#[cfg(test)]
mod tests {
    #[cfg(all(windows, feature = "test-script"))]
    #[test]
    fn click_widget_reaches_an_egui_button_handler() {
        let (reply, received) = std::sync::mpsc::sync_channel(1);
        super::request_widget_click("Test button".to_string(), reply).unwrap();
        let ctx = egui::Context::default();
        let render = |mut input: egui::RawInput| {
            input.screen_rect = Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 300.0),
            ));
            ctx.begin_pass(input);
            let mut clicked = false;
            egui::CentralPanel::default().show(&ctx, |ui| {
                let response = ui.button("Test button");
                super::register_clickable_widget("Test button", &response);
                clicked = response.clicked();
            });
            let _ = ctx.end_pass();
            clicked
        };

        assert!(!render(egui::RawInput {
            time: Some(0.0),
            focused: true,
            ..Default::default()
        }));
        received
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap()
            .unwrap();
        let mut down = egui::RawInput {
            time: Some(0.01),
            focused: true,
            ..Default::default()
        };
        super::append_widget_click_events(&mut down);
        assert!(!render(down));
        let mut up = egui::RawInput {
            time: Some(0.02),
            focused: true,
            ..Default::default()
        };
        super::append_widget_click_events(&mut up);
        assert!(render(up));
    }

    #[test]
    fn rating_sort_smoke_script_parses_without_launching_the_app() {
        let script = include_str!("../scripts/ui-smoke/rating-sort.rhai");
        let mut engine = rhai::Engine::new();
        engine.set_max_expr_depths(64, 64);
        engine.compile(script).unwrap();
    }

    #[test]
    fn collection_sort_smoke_script_parses_without_launching_the_app() {
        let script = include_str!("../scripts/ui-smoke/rating-sort-collection.rhai");
        let mut engine = rhai::Engine::new();
        engine.set_max_expr_depths(64, 64);
        engine.compile(script).unwrap();
    }

    #[test]
    fn always_on_top_smoke_script_parses_without_launching_the_app() {
        let mut engine = rhai::Engine::new();
        engine.set_max_expr_depths(64, 64);
        engine
            .compile(include_str!("../scripts/ui-smoke/always-on-top.rhai"))
            .unwrap();
        engine
            .compile(include_str!(
                "../scripts/ui-smoke/always-on-top-restart.rhai"
            ))
            .unwrap();
    }

    #[test]
    fn always_on_top_literal_calls_execute_in_the_runner() {
        let scenario = include_str!("../scripts/ui-smoke/always-on-top.rhai");
        let call_lines = |name: &str| -> Vec<&str> {
            let prefix = format!("{name}(\"");
            scenario
                .lines()
                .map(str::trim)
                .filter(|line| line.starts_with(&prefix))
                .collect()
        };
        let keys = call_lines("tap_key");
        let actions = call_lines("run_action");
        let smoke = call_lines("always_on_top_smoke");
        let captures = call_lines("capture");
        assert_eq!(
            keys,
            [
                "tap_key(\"F12\");",
                "tap_key(\"F12\");",
                "tap_key(\"Right\");"
            ]
        );
        assert!(!actions.is_empty() && !smoke.is_empty() && !captures.is_empty());

        let source = keys
            .iter()
            .chain(actions.iter())
            .chain(smoke.iter())
            .chain(captures.iter())
            .copied()
            .collect::<Vec<_>>()
            .join("\n");
        let (bridge, rx, _) = runner_bridge(ready_snapshot());
        spawn_script_source(source, bridge).unwrap();
        let (mut key_downs, mut action_count, mut smoke_count, mut capture_count) = (0, 0, 0, 0);
        loop {
            match rx
                .recv_timeout(Duration::from_secs(2))
                .expect("runner command")
            {
                UiCommand::Key(SyntheticKeyCommand {
                    kind: SyntheticKeyCommandKind::Down { .. },
                    ..
                }) => key_downs += 1,
                UiCommand::RunAction { applied, .. } => {
                    action_count += 1;
                    applied.send(Ok(())).unwrap();
                }
                UiCommand::SmokeAction(_) => smoke_count += 1,
                UiCommand::Capture { reply, .. } => {
                    capture_count += 1;
                    reply.send(Ok(())).unwrap();
                }
                UiCommand::Finished(outcome) => {
                    assert_eq!(
                        outcome.kind,
                        ScriptOutcomeKind::Success,
                        "{}",
                        outcome.message
                    );
                    break;
                }
                _ => {}
            }
        }
        assert_eq!(key_downs, keys.len());
        assert_eq!(action_count, actions.len());
        assert_eq!(smoke_count, smoke.len());
        assert_eq!(capture_count, captures.len());
    }

    #[test]
    fn details_preview_move_primitive_is_registered_with_the_runner() {
        let (bridge, rx, _) = runner_bridge(ready_snapshot());
        spawn_script_source("move_details_preview_pointer(1);".to_string(), bridge).unwrap();
        let commands = receive_through_finished(&rx);
        assert!(matches!(
            commands.last(),
            Some(UiCommand::Finished(ScriptOutcome {
                kind: ScriptOutcomeKind::ScriptFailure,
                message,
            })) if message.contains("no live root host")
        ));
    }

    #[test]
    fn details_preview_pointer_uses_the_visible_widget_rect() {
        let (reply, received) = mpsc::sync_channel(1);
        WIDGET_CLICK_DRIVER.with(|driver| driver.borrow_mut().details_preview_move = Some(reply));
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 300.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(60.0, 24.0), egui::Sense::hover());
                super::register_details_preview_pointer(
                    &response,
                    egui::Rect::from_min_max(
                        rect.min,
                        egui::pos2(rect.center().x - 1.0, rect.max.y),
                    ),
                );
                assert!(
                    received.try_recv().is_err(),
                    "clipped preview must not be targeted"
                );
                super::register_details_preview_pointer(&response, ui.clip_rect());
                let (point, ppp) = received.try_recv().unwrap().unwrap();
                assert_eq!(point, rect.center());
                assert_eq!(ppp, ctx.pixels_per_point());
            });
        });
        WIDGET_CLICK_DRIVER.with(|driver| assert!(driver.borrow().details_preview_move.is_none()));
    }

    #[test]
    fn multi_window_pdf_script_with_captures_parses() {
        let mut engine = rhai::Engine::new();
        engine.set_max_expr_depths(64, 64);
        engine
            .compile(include_str!("../scripts/ui-smoke/multi-window-pdf.rhai"))
            .expect("PDF smoke script syntax");
    }

    #[test]
    fn disabled_sort_row_receives_pointer_events_without_a_click() {
        let (reply, received) = std::sync::mpsc::sync_channel(1);
        super::request_widget_pointer(
            "評価↑".to_string(),
            super::WidgetPointerRequestKind::SortPopup(super::SortPopupPointerKind::Click),
            reply,
        )
        .unwrap();
        let ctx = egui::Context::default();
        let render = |mut input: egui::RawInput| {
            input.screen_rect = Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 300.0),
            ));
            ctx.begin_pass(input);
            let mut clicked = false;
            egui::CentralPanel::default().show(&ctx, |ui| {
                let response = ui.add_enabled(false, egui::Button::new("評価↑"));
                super::register_sort_popup_pointer_row("評価↑", &response, ui.clip_rect());
                clicked = response.clicked();
            });
            let _ = ctx.end_pass();
            clicked
        };
        assert!(!render(egui::RawInput {
            time: Some(0.0),
            focused: true,
            ..Default::default()
        }));
        received
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        for time in [0.01, 0.02] {
            let mut input = egui::RawInput {
                time: Some(time),
                focused: true,
                ..Default::default()
            };
            super::append_widget_click_events(&mut input);
            assert_eq!(input.events.len(), 2);
            assert!(!render(input));
        }
    }

    #[test]
    fn disabled_sort_row_hover_uses_a_pointer_move_without_a_click() {
        let (reply, received) = std::sync::mpsc::sync_channel(1);
        super::request_widget_pointer(
            "評価↑".to_string(),
            super::WidgetPointerRequestKind::SortPopup(super::SortPopupPointerKind::Hover),
            reply,
        )
        .unwrap();
        let ctx = egui::Context::default();
        let render = |mut input: egui::RawInput| {
            input.screen_rect = Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 300.0),
            ));
            ctx.begin_pass(input);
            let mut contains_pointer = false;
            egui::CentralPanel::default().show(&ctx, |ui| {
                let response = ui.add_enabled(false, egui::Button::new("評価↑"));
                super::register_sort_popup_pointer_row("評価↑", &response, ui.clip_rect());
                contains_pointer = response.contains_pointer();
                assert!(!response.clicked());
            });
            let _ = ctx.end_pass();
            contains_pointer
        };
        assert!(!render(egui::RawInput {
            time: Some(0.0),
            focused: true,
            ..Default::default()
        }));
        received
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        let mut input = egui::RawInput {
            time: Some(0.01),
            focused: true,
            ..Default::default()
        };
        super::append_widget_click_events(&mut input);
        assert_eq!(input.events.len(), 1);
        assert!(matches!(input.events[0], egui::Event::PointerMoved(_)));
        assert!(render(input));
    }

    #[test]
    fn collection_sort_popup_readback_tracks_visible_disabled_rows_and_expires() {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 300.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(input.clone(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                super::begin_collection_sort_popup(ctx, false, ui.is_enabled());
                let manual = ui.button("手動");
                let rating = ui.add_enabled(false, egui::Button::new("評価↑"));
                let clip = egui::Rect::from_min_max(
                    manual.rect.min - egui::vec2(1.0, 1.0),
                    manual.rect.max + egui::vec2(1.0, 1.0),
                );
                super::record_collection_sort_popup_row("手動", &manual, clip);
                super::record_collection_sort_popup_row("評価↑", &rating, clip);
            });
        });
        let observed = super::collection_sort_popup_snapshot(&ctx);
        assert!(observed.open);
        assert!(observed.needs_scrolling);
        assert!(observed.within_screen);
        assert_eq!(observed.rows.len(), 2);
        assert!(observed.rows[0].visible);
        assert!(observed.rows[0].visible_x && observed.rows[0].visible_y);
        assert!(observed.popup_ui_enabled);
        assert!(!observed.sort_control_locked);
        assert!(!observed.rows[0].disabled);
        assert!(!observed.rows[1].visible);
        assert!(!observed.rows[1].visible_y);
        assert!(observed.rows[1].disabled);
        assert!(observed.rendered_tooltip.is_none());
        for _ in 0..2 {
            let _ = ctx.run(input.clone(), |_| {});
        }
        assert!(!super::collection_sort_popup_snapshot(&ctx).open);
    }

    #[test]
    fn collection_sort_popup_horizontal_clip_is_not_scrolling() {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 300.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                super::begin_collection_sort_popup(ctx, false, ui.is_enabled());
                let row = ui.button("シャッフル（再選択で並べ直す）");
                let clip = egui::Rect::from_min_max(
                    row.rect.min - egui::vec2(1.0, 1.0),
                    egui::pos2(row.rect.center().x, row.rect.max.y + 1.0),
                );
                super::record_collection_sort_popup_row(
                    "シャッフル（再選択で並べ直す）",
                    &row,
                    clip,
                );
            });
        });
        let popup = super::collection_sort_popup_snapshot(&ctx);
        assert!(!popup.needs_scrolling);
        assert!(!popup.rows[0].visible);
        assert!(!popup.rows[0].visible_x);
        assert!(popup.rows[0].visible_y);
        assert!(!popup.rows[0].disabled);
        assert!(popup.rows[0].geometry.contains("clip="));
    }

    use super::InterruptState;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn multi_window_rar_nav_script_parses() {
        rhai::Engine::new()
            .compile(include_str!(
                "../scripts/ui-smoke/multi-window-rar-nav.rhai"
            ))
            .expect("RAR navigation smoke script syntax");
    }

    /// A hold must last as long as it was asked to, even when something wakes the condvar.
    ///
    /// Without the loop this returned on the first notify and a twenty-second burst finished in
    /// 45ms, so the scenario never ran and the script reported success anyway.
    #[test]
    fn a_wait_is_not_ended_by_a_wakeup_that_is_not_a_failure() {
        let state = Arc::new(InterruptState::default());
        let waker = Arc::clone(&state);
        std::thread::spawn(move || {
            for _ in 0..10 {
                std::thread::sleep(Duration::from_millis(5));
                waker.changed.notify_all();
            }
        });
        let started = Instant::now();
        state
            .wait(Duration::from_millis(300))
            .expect("no failure was set");
        let elapsed = started.elapsed();
        assert!(
            elapsed >= Duration::from_millis(290),
            "the wait ended after {elapsed:?} instead of running its full 300ms"
        );
    }

    #[test]
    fn a_wait_ends_as_soon_as_the_run_fails() {
        let state = Arc::new(InterruptState::default());
        let failer = Arc::clone(&state);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            failer.fail("the window lost focus");
        });
        let started = Instant::now();
        let error = state
            .wait(Duration::from_secs(30))
            .expect_err("a failure must interrupt the wait");
        assert_eq!(error, "the window lost focus");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a failure must not wait out the full duration"
        );
    }

    use super::*;
    use std::ffi::OsString;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn runner_bridge(
        snapshot: TestScriptSnapshot,
    ) -> (RunnerBridge, mpsc::Receiver<UiCommand>, Arc<AtomicUsize>) {
        let (tx, rx) = mpsc::channel();
        let wakes = Arc::new(AtomicUsize::new(0));
        let wake_count = Arc::clone(&wakes);
        (
            RunnerBridge {
                tx,
                snapshot: Arc::new(RwLock::new(snapshot)),
                interrupt: Arc::new(InterruptState::default()),
                wake: Arc::new(move || {
                    wake_count.fetch_add(1, AtomicOrdering::Relaxed);
                }),
                next_hold_id: Arc::new(AtomicU64::new(0)),
                action_selection: Arc::new(Mutex::new(TestScriptActionSelection::LegacyImplicit)),
                pointer_regions: pointer_input::new_shared_catalog(),
            },
            rx,
            wakes,
        )
    }

    fn ready_snapshot() -> TestScriptSnapshot {
        TestScriptSnapshot {
            focused: true,
            target_registered: true,
            target_viewport: "ROOT".to_string(),
            ..TestScriptSnapshot::default()
        }
    }

    fn receive_through_finished(rx: &mpsc::Receiver<UiCommand>) -> Vec<UiCommand> {
        let mut commands = Vec::new();
        loop {
            let command = rx
                .recv_timeout(Duration::from_secs(2))
                .expect("runner command");
            let finished = matches!(command, UiCommand::Finished(_));
            commands.push(command);
            if finished {
                return commands;
            }
        }
    }

    #[test]
    fn cli_requires_isolated_data_dir() {
        let parsed = cli_script_path_from(&args(&[
            "mimageviewer-core.exe",
            "--test-script",
            "smoke.rhai",
        ]));
        assert!(parsed.unwrap_err().contains("--data-dir"));
    }

    #[test]
    fn cli_returns_script_and_does_not_treat_its_value_as_a_path_argument() {
        let parsed = cli_script_path_from(&args(&[
            "mimageviewer-core.exe",
            "--data-dir",
            "sandbox",
            "--test-script",
            "smoke.rhai",
        ]))
        .unwrap();
        assert_eq!(parsed, Some(PathBuf::from("smoke.rhai")));
    }

    #[test]
    fn cli_capture_dir_is_required_and_kept_with_the_script() {
        let missing = cli_capture_dir_from(&args(&[
            "mimageviewer-core.exe",
            "--data-dir",
            "sandbox",
            "--test-script",
            "smoke.rhai",
        ]));
        assert!(missing.unwrap_err().contains("--test-evidence-dir"));
        let parsed = cli_capture_dir_from(&args(&[
            "mimageviewer-core.exe",
            "--data-dir",
            "sandbox",
            "--test-script",
            "smoke.rhai",
            "--test-evidence-dir",
            "run-evidence",
        ]))
        .unwrap();
        assert_eq!(parsed, Some(PathBuf::from("run-evidence")));
        assert!(
            cli_capture_dir_from(&args(&[
                "mimageviewer-core.exe",
                "--test-evidence-dir",
                "run-evidence",
            ]))
            .is_err()
        );
    }

    #[test]
    fn capture_script_call_waits_for_the_ui_reply() {
        let (bridge, rx, _) = runner_bridge(ready_snapshot());
        spawn_script_source("capture(\"checkpoint\");".into(), bridge).unwrap();
        let command = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let UiCommand::Capture {
            label,
            scope,
            selection,
            reply,
        } = command
        else {
            panic!("expected capture command");
        };
        assert_eq!(label, "checkpoint");
        assert_eq!(scope, CaptureScope::Selected);
        assert!(matches!(
            selection,
            TestScriptActionSelection::LegacyImplicit
        ));
        reply.send(Ok(())).unwrap();
        let commands = receive_through_finished(&rx);
        assert!(
            matches!(commands.last(), Some(UiCommand::Finished(outcome)) if outcome.kind == ScriptOutcomeKind::Success)
        );
    }

    #[test]
    fn failure_capture_watchdog_exits_without_another_frame() {
        const CHILD_ENV: &str = "MIV_TEST_FAILURE_CAPTURE_WATCHDOG_RUN";
        const ORIGINAL_MESSAGE: &str = "original failure before screenshot";
        if let Some(run_dir) = std::env::var_os(CHILD_ENV) {
            let (_tx, rx) = mpsc::channel();
            let mut runtime = UiRuntime::new(
                rx,
                Arc::new(RwLock::new(TestScriptSnapshot::default())),
                Arc::new(InterruptState::default()),
                Arc::new(RwLock::new(pointer_input::RegionCatalog::default())),
            );
            runtime.capture = Some(capture::Coordinator::new(Path::new(&run_dir)).unwrap());
            runtime.begin_finish(ScriptOutcome::script_failure(ORIGINAL_MESSAGE), 1);
            let ctx = egui::Context::default();
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                runtime.request_failure_capture(ctx, |outcome| {
                    assert_eq!(outcome.message, ORIGINAL_MESSAGE);
                    PROCESS_EXIT_CODE.store(outcome.kind.exit_code(), Ordering::Release);
                    println!("failure-capture original message={}", outcome.message);
                    std::io::Write::flush(&mut std::io::stdout()).unwrap();
                    arm_shutdown_watchdog(outcome.kind.exit_code(), "failure-capture-test");
                });
            });
            assert!(runtime.capture.as_ref().unwrap().is_pending(1));
            // No second ctx.run or capture.poll: only the process watchdog can
            // complete this child. The parent kills it if that guarantee fails.
            std::thread::sleep(SHUTDOWN_WATCHDOG_GRACE + Duration::from_secs(3));
            panic!("watchdog did not terminate a stalled capture");
        }

        let runs_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("ui-smoke-runs");
        std::fs::create_dir_all(&runs_root).unwrap();
        let run = tempfile::Builder::new()
            .prefix("watchdog-capture-test-")
            .tempdir_in(runs_root)
            .unwrap();
        let started = Instant::now();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("test_script::tests::failure_capture_watchdog_exits_without_another_frame")
            .arg("--nocapture")
            .env(CHILD_ENV, run.path())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = started + SHUTDOWN_WATCHDOG_GRACE + Duration::from_secs(2);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("failure capture did not exit within the watchdog budget");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(EXIT_SCRIPT_FAILURE));
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(ORIGINAL_MESSAGE),
            "original failure message missing from child output"
        );
    }

    #[test]
    fn script_thread_translates_calls_to_typed_commands_and_wakes() {
        let (bridge, rx, wakes) = runner_bridge(ready_snapshot());
        spawn_script_source(
            r#"
                tap_key("Right");
                hold_key("Left", 1);
                release_key("Home");
                set_repeat(125, 20);
                log("hello");
            "#
            .to_string(),
            bridge,
        )
        .unwrap();
        let commands = receive_through_finished(&rx);
        let key_kinds = commands
            .iter()
            .filter_map(|command| match command {
                UiCommand::Key(command) => Some(command.kind),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            key_kinds,
            vec![
                SyntheticKeyCommandKind::Down {
                    key: SyntheticNavigationKey::Right,
                    modifiers: SyntheticModifiers::default(),
                },
                SyntheticKeyCommandKind::Up {
                    key: SyntheticNavigationKey::Right,
                },
                SyntheticKeyCommandKind::Down {
                    key: SyntheticNavigationKey::Left,
                    modifiers: SyntheticModifiers::default(),
                },
                SyntheticKeyCommandKind::Up {
                    key: SyntheticNavigationKey::Left,
                },
                SyntheticKeyCommandKind::Up {
                    key: SyntheticNavigationKey::Home,
                },
            ]
        );
        assert!(commands.iter().any(|command| matches!(
            command,
            UiCommand::SetRepeat { delay, hz }
                if *delay == Duration::from_millis(125) && (*hz - 20.0).abs() < f64::EPSILON
        )));
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, UiCommand::Log(message) if message == "hello"))
        );
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, UiCommand::Cancel(_)))
        );
        assert!(matches!(
            commands.last(),
            Some(UiCommand::Finished(ScriptOutcome {
                kind: ScriptOutcomeKind::Success,
                ..
            }))
        ));
        assert_eq!(wakes.load(AtomicOrdering::Relaxed), commands.len());
    }

    #[test]
    fn invalid_key_is_a_script_failure_instead_of_a_noop() {
        let (bridge, rx, _) = runner_bridge(ready_snapshot());
        spawn_script_source(r#"tap_key("A");"#.to_string(), bridge).unwrap();
        let commands = receive_through_finished(&rx);
        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, UiCommand::Key(_)))
        );
        assert!(matches!(
            commands.last(),
            Some(UiCommand::Finished(ScriptOutcome {
                kind: ScriptOutcomeKind::ScriptFailure,
                message,
            })) if message.contains("unsupported synthetic navigation key")
        ));
    }

    #[test]
    fn hold_key_assigns_one_monotonic_id_to_its_down_and_up() {
        let (bridge, rx, _) = runner_bridge(ready_snapshot());
        spawn_script_source(
            r#"
                hold_key("Left", 0);
                hold_key("Right", 0);
            "#
            .to_string(),
            bridge,
        )
        .unwrap();
        let commands = receive_through_finished(&rx);
        let hold_ids = commands
            .iter()
            .filter_map(|command| match command {
                UiCommand::Key(command) => command.hold_id,
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(hold_ids, [1, 1, 2, 2]);
    }

    #[test]
    fn run_action_translates_ini_name_and_waits_for_ui_acknowledgement() {
        let (bridge, rx, wakes) = runner_bridge(ready_snapshot());
        let action = KeyAction::FsClose;
        spawn_script_source(format!(r#"run_action("{}");"#, action.ini_name()), bridge).unwrap();

        match rx.recv_timeout(Duration::from_secs(2)).unwrap() {
            UiCommand::RunAction {
                action: actual,
                selection,
                applied,
            } => {
                assert_eq!(actual, action);
                assert_eq!(selection, TestScriptActionSelection::LegacyImplicit);
                applied.send(Ok(())).unwrap();
            }
            command => panic!("unexpected command before action acknowledgement: {command:?}"),
        }

        let commands = receive_through_finished(&rx);
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, UiCommand::Cancel(_)))
        );
        assert!(matches!(
            commands.last(),
            Some(UiCommand::Finished(ScriptOutcome {
                kind: ScriptOutcomeKind::Success,
                ..
            }))
        ));
        assert_eq!(
            wakes.load(AtomicOrdering::Relaxed),
            commands.len() + 1,
            "RunAction, Cancel, and Finished must each wake ROOT"
        );
    }

    #[test]
    fn selected_root_is_attached_to_run_action_as_an_exact_target() {
        let owner = TestScriptWindowIdentity::Root {
            context_serial: 31,
            hwnd: 0x3131,
            backend_token: 41,
        };
        let mut snapshot = ready_snapshot();
        snapshot.windows = vec![window_snapshot(owner.clone(), 1, 0, "root-item")];
        let (bridge, rx, _) = runner_bridge(snapshot);
        spawn_script_source(
            r#"
                let selected = select_root();
                if selected.target_mode != "targeted" || !selected.current {
                    fail("root target missing");
                }
                run_action("GridMoveFirst");
            "#
            .to_string(),
            bridge,
        )
        .unwrap();

        loop {
            match rx.recv_timeout(Duration::from_secs(2)).unwrap() {
                UiCommand::RunAction {
                    action,
                    selection,
                    applied,
                } => {
                    assert_eq!(action, KeyAction::GridMoveFirst);
                    assert_eq!(selection, TestScriptActionSelection::Targeted(owner));
                    applied.send(Ok(())).unwrap();
                    break;
                }
                UiCommand::Log(message) => assert!(message.contains("mode=targeted")),
                command => panic!("unexpected command before targeted action: {command:?}"),
            }
        }
        assert!(matches!(
            receive_through_finished(&rx).last(),
            Some(UiCommand::Finished(ScriptOutcome {
                kind: ScriptOutcomeKind::Success,
                ..
            }))
        ));
    }

    #[test]
    fn explicit_selection_fails_instead_of_reserving_a_missing_identity() {
        let (bridge, rx, _) = runner_bridge(ready_snapshot());
        spawn_script_source("select_root();".to_string(), bridge).unwrap();
        let commands = receive_through_finished(&rx);

        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, UiCommand::RunAction { .. }))
        );
        assert!(matches!(
            commands.last(),
            Some(UiCommand::Finished(ScriptOutcome {
                kind: ScriptOutcomeKind::ScriptFailure,
                message,
            })) if message.contains("could not find the root window")
        ));
    }

    #[test]
    fn pending_action_peek_is_repeatable_within_the_ui_frame() {
        let action = KeyAction::FsClose;
        let (applied, acknowledgement) = mpsc::sync_channel(1);
        let mut pending = VecDeque::from([PendingAction {
            action,
            dispatch: PendingActionDispatch::LegacyImplicit,
            applied: Some(applied),
        }]);

        assert!(peek_pending_action_from(&mut pending, None, action));
        assert_eq!(
            acknowledgement
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            Ok(())
        );
        assert!(peek_pending_action_from(&mut pending, None, action));
        assert!(matches!(
            acknowledgement.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        assert!(consume_pending_action_from(&mut pending, None, action));
        assert!(pending.is_empty());
    }

    #[test]
    fn fail_api_finishes_nonzero() {
        let (bridge, rx, _) = runner_bridge(ready_snapshot());
        spawn_script_source(r#"fail("expected");"#.to_string(), bridge).unwrap();
        let commands = receive_through_finished(&rx);
        assert!(matches!(
            commands.last(),
            Some(UiCommand::Finished(ScriptOutcome {
                kind: ScriptOutcomeKind::ScriptFailure,
                message,
            })) if message.contains("expected")
        ));
    }

    #[cfg(feature = "test-script")]
    #[test]
    fn invalid_native_mouse_arguments_are_script_failures() {
        let (bridge, rx, _) = runner_bridge(ready_snapshot());
        spawn_script_source("move_native_canvas(0.0, 0.5, 1000);".to_string(), bridge).unwrap();
        let commands = receive_through_finished(&rx);
        assert!(matches!(
            commands.last(),
            Some(UiCommand::Finished(ScriptOutcome {
                kind: ScriptOutcomeKind::ScriptFailure,
                message,
            })) if message.contains("coordinates")
        ));
    }

    #[cfg(feature = "test-script")]
    #[test]
    fn native_top_panorama_rejects_a_zero_timeout_before_any_ui_input() {
        let (bridge, rx, _) = runner_bridge(ready_snapshot());
        spawn_script_source("reveal_native_top_panorama(0);".to_string(), bridge).unwrap();
        let commands = receive_through_finished(&rx);
        assert!(matches!(
            commands.last(),
            Some(UiCommand::Finished(ScriptOutcome {
                kind: ScriptOutcomeKind::ScriptFailure,
                message,
            })) if message.contains("greater than zero")
        ));
        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, UiCommand::RunAction { .. } | UiCommand::Key(_)))
        );
    }

    #[cfg(feature = "test-script")]
    #[test]
    fn native_top_panorama_click_rejects_a_zero_timeout_before_any_ui_input() {
        let (bridge, rx, _) = runner_bridge(ready_snapshot());
        spawn_script_source("click_native_top_panorama(0);".to_string(), bridge).unwrap();
        let commands = receive_through_finished(&rx);
        assert!(matches!(
            commands.last(),
            Some(UiCommand::Finished(ScriptOutcome {
                kind: ScriptOutcomeKind::ScriptFailure,
                message,
            })) if message.contains("greater than zero")
        ));
        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, UiCommand::RunAction { .. } | UiCommand::Key(_)))
        );
    }

    #[cfg(feature = "test-script")]
    #[test]
    fn native_top_panorama_hover_scenario_compiles_with_the_registered_api() {
        let (bridge, _, _) = runner_bridge(ready_snapshot());
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("scripts/ui-smoke/native-top-panorama-hover.rhai"),
        )
        .unwrap();
        build_engine(bridge).compile(&source).unwrap();
    }

    #[cfg(feature = "test-script")]
    #[test]
    fn multi_window_stills_scenario_compiles_with_the_registered_api() {
        let (bridge, _, _) = runner_bridge(ready_snapshot());
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("scripts/ui-smoke/multi-window-stills.rhai"),
        )
        .unwrap();
        build_engine(bridge).compile(&source).unwrap();
    }

    #[cfg(feature = "test-script")]
    #[test]
    fn drawn_paint_records_each_submitted_mesh_with_resource_provenance() {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "paint-observation",
            egui::ColorImage::filled([1, 1], egui::Color32::WHITE),
            egui::TextureOptions::NEAREST,
        );
        let make = |item: &str| {
            crate::gpu_lanczos::FullscreenPaintResource::direct(texture.clone())
                .with_test_script_content_proof(TestScriptContentProof {
                    context_serial: 7,
                    items_generation: 3,
                    page_index: 0,
                    item_identity: item.into(),
                    source_texture_id: texture.id(),
                    source_kind: TestScriptPaintSourceKind::FullOrProcessed,
                    final_composite_complete: false,
                })
        };
        let first = make("folder");
        let second = make("zip");
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::background());
            for (index, resource) in [&first, &second].into_iter().enumerate() {
                let rect = egui::Rect::from_min_size(
                    egui::pos2(index as f32 * 20.0, 0.0),
                    egui::vec2(10.0, 15.0),
                );
                painter.image(
                    resource.paint_texture_id(),
                    rect,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
                record_drawn_paint(&painter, resource, "normal".into());
            }
        });
        let paints = take_drawn_paint(egui::ViewportId::ROOT);
        assert_eq!(paints.len(), 2);
        assert_eq!(paints[0].content.item_identity, "folder");
        assert_eq!(paints[1].content.item_identity, "zip");
        assert_ne!(paints[0].vertices[0].0, paints[1].vertices[0].0);
        assert!(take_drawn_paint(egui::ViewportId::ROOT).is_empty());
    }

    #[test]
    fn sidecar_import_evidence_is_tied_to_context_and_items_generation() {
        let owner = window_identity(7, 11, 13);
        let current = window_snapshot(owner, 17, 0, "folder::page");
        let imports = std::collections::HashSet::from([
            (11, 17, TestScriptSidecarObservation::Loaded),
            (11, 17, TestScriptSidecarObservation::Imported),
        ]);
        let joined = joined_window_snapshots(
            &[current.clone()],
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &imports,
        );
        assert!(joined[0].sidecar_imported);
        assert!(joined[0].sidecar_loaded);
        let changed = window_snapshot(window_identity(7, 11, 13), 18, 0, "folder::page");
        let joined = joined_window_snapshots(
            &[changed],
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &imports,
        );
        assert!(!joined[0].sidecar_imported);
        assert!(!joined[0].sidecar_loaded);

        let loaded_only =
            std::collections::HashSet::from([(11, 17, TestScriptSidecarObservation::Loaded)]);
        let joined = joined_window_snapshots(
            &[current],
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &loaded_only,
        );
        assert!(joined[0].sidecar_loaded);
        assert!(!joined[0].sidecar_imported);
    }

    #[test]
    fn frame_paints_publish_without_legacy_single_page_proof_and_clear_next_frame() {
        let mut runtime = local_runtime();
        let owner = window_identity(7, 11, 13);
        runtime
            .publish_windows(vec![window_snapshot(owner.clone(), 17, 0, "folder::page")])
            .unwrap();
        DRAWN_PAINT.with(|drawn| {
            drawn.borrow_mut().insert(
                owner.viewport_id(),
                vec![PendingPaintObservation {
                    content: content_proof(
                        11,
                        17,
                        0,
                        "folder::page",
                        19,
                        TestScriptPaintSourceKind::FullOrProcessed,
                    ),
                    texture: egui::TextureId::Managed(19),
                    vertices: vec![([1.0, 2.0], [0.0, 0.0])],
                    clip: [0.0, 0.0, 10.0, 10.0],
                    viewport_size: [10.0, 10.0],
                    pixels_per_point: 1.0,
                    placement: "frozen".into(),
                }],
            );
        });
        runtime.publish_window_frame(owner.clone(), None).unwrap();
        let first = runtime.joined_windows();
        assert_eq!(first[0].paints.len(), 1);
        assert!(!first[0].paint_matches_current_page);
        runtime.publish_window_frame(owner, None).unwrap();
        assert!(runtime.joined_windows()[0].paints.is_empty());
    }

    #[cfg(feature = "test-script")]
    #[test]
    fn native_top_panorama_click_scenario_compiles_with_the_registered_api() {
        let (bridge, _, _) = runner_bridge(ready_snapshot());
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("scripts/ui-smoke/native-top-panorama-click.rhai"),
        )
        .unwrap();
        build_engine(bridge).compile(&source).unwrap();
    }

    #[cfg(feature = "test-script")]
    #[test]
    fn native_seek_strip_whole_lifecycle_scenario_compiles_with_the_registered_api() {
        let (bridge, _, _) = runner_bridge(ready_snapshot());
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("scripts/ui-smoke/native-seek-strip-whole-lifecycle.rhai"),
        )
        .unwrap();
        build_engine(bridge).compile(&source).unwrap();
    }

    #[cfg(feature = "test-script")]
    #[test]
    fn native_seek_strip_fixture_exceeds_the_nine_cell_fallback_at_a_bounded_width() {
        let cell_height = crate::video::seek_strip_layout::SeekStripHeightValues::default()
            .points(crate::video::seek_strip_layout::SeekStripHeight::Smallest)
            - crate::video::seek_strip_layout::SEEK_STRIP_CELL_VERTICAL_INSET;
        let count = crate::video::seek_strip_layout::whole_cell_count(
            400.0,
            cell_height,
            Some(720.0 / 576.0),
        );
        assert!(count > 9, "the S4 fixture must distinguish the fallback");
    }

    #[cfg(feature = "test-script")]
    #[test]
    fn native_mouse_runtime_failures_are_environment_failures() {
        let (bridge, rx, _) = runner_bridge(ready_snapshot());
        spawn_script_source("move_native_canvas(0.5, 0.5, 1000);".to_string(), bridge).unwrap();
        let commands = receive_through_finished(&rx);
        assert!(matches!(
            commands.last(),
            Some(UiCommand::Finished(ScriptOutcome {
                kind: ScriptOutcomeKind::EnvironmentFailure,
                message,
            })) if message.contains("requires select_window")
        ));
    }

    #[test]
    fn wait_until_reads_published_snapshot_across_thread_boundary() {
        let (bridge, rx, _) = runner_bridge(TestScriptSnapshot::default());
        let snapshot = Arc::clone(&bridge.snapshot);
        spawn_script_source(
            "wait_until(|s| s.is_fullscreen && s.focused, 1000);".to_string(),
            bridge,
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        {
            let mut published = snapshot.write().unwrap();
            published.is_fullscreen = true;
            published.focused = true;
        }
        let commands = receive_through_finished(&rx);
        assert!(matches!(
            commands.last(),
            Some(UiCommand::Finished(ScriptOutcome {
                kind: ScriptOutcomeKind::Success,
                ..
            }))
        ));
    }

    #[test]
    fn issue_classification_names_environment_failures() {
        let viewport = egui::ViewportId::from_hash_of("missing-child");
        let issue = SyntheticInputIssue::TargetViewportNotRendered {
            viewport,
            raw_input_time: Some(1.5),
            event_count: 3,
        };
        let message = describe_issue(&issue);
        assert!(message.contains("not rendered"));
        assert!(message.contains("event_count=3"));
    }

    fn window_identity(
        window_id: u64,
        context_serial: u64,
        host_incarnation: u64,
    ) -> TestScriptWindowIdentity {
        window_identity_with_backend_token(
            window_id,
            context_serial,
            host_incarnation,
            0x2000 + host_incarnation,
        )
    }

    fn window_identity_with_backend_token(
        window_id: u64,
        context_serial: u64,
        host_incarnation: u64,
        backend_token: u64,
    ) -> TestScriptWindowIdentity {
        TestScriptWindowIdentity::Detached {
            window_id,
            context_serial,
            viewport_id: egui::ViewportId::from_hash_of(("test-window", window_id)),
            host_incarnation,
            hwnd: 0x1000 + host_incarnation,
            backend_token,
        }
    }

    fn root_identity(context_serial: u64, hwnd: u64) -> TestScriptWindowIdentity {
        TestScriptWindowIdentity::Root {
            context_serial,
            hwnd,
            backend_token: 0x3000 + context_serial,
        }
    }

    fn detached_identity_from_witness(
        witness: eframe::miv_test_script_window_witness::WindowWitness,
        context_serial: u64,
        host_incarnation: u64,
    ) -> TestScriptWindowIdentity {
        TestScriptWindowIdentity::Detached {
            window_id: 7,
            context_serial,
            viewport_id: witness.viewport_id(),
            host_incarnation,
            hwnd: witness.hwnd(),
            backend_token: witness.token(),
        }
    }

    fn local_runtime() -> UiRuntime {
        let (_tx, rx) = mpsc::channel();
        UiRuntime::new(
            rx,
            Arc::new(RwLock::new(TestScriptSnapshot::default())),
            Arc::new(InterruptState::default()),
            pointer_input::new_shared_catalog(),
        )
    }

    #[test]
    fn capture_targets_preserve_frozen_presentation_and_native_registration() {
        let root = root_identity(0, 0x100);
        let first = window_identity(1, 1, 1);
        let second = window_identity(2, 2, 2);
        let mut runtime = local_runtime();
        let mut frozen = window_snapshot(first.clone(), 1, 0, "pdf::first");
        frozen.presentation = TestScriptWindowPresentation::PassiveDeferredFrozen;
        runtime
            .publish_windows(vec![
                window_snapshot(root, 1, 0, "root::page"),
                frozen,
                window_snapshot(second.clone(), 1, 0, "pdf::second"),
            ])
            .unwrap();

        // Native registration and App presentation are separate facts. A live
        // native host can hold a frozen passive view with no egui pass.
        let native_viewports = std::collections::HashSet::from([
            egui::ViewportId::ROOT,
            first.viewport_id(),
            second.viewport_id(),
        ]);
        let targets = runtime
            .capture_targets(
                CaptureScope::All,
                &TestScriptActionSelection::LegacyImplicit,
                &native_viewports,
            )
            .unwrap();
        assert_eq!(targets.len(), 3);
        assert_eq!(targets[1].viewport_id, first.viewport_id());
        assert_eq!(targets[1].availability, capture::Availability::Registered);
        assert_eq!(
            targets[1].presentation,
            TestScriptWindowPresentation::PassiveDeferredFrozen
        );
        assert_eq!(
            targets[2].presentation,
            TestScriptWindowPresentation::ActiveImmediate
        );

        let selected = runtime
            .capture_targets(
                CaptureScope::Selected,
                &TestScriptActionSelection::Targeted(first.clone()),
                &native_viewports,
            )
            .unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(
            selected[0].presentation,
            TestScriptWindowPresentation::PassiveDeferredFrozen
        );

        // If eframe no longer owns that viewport, keep its identity in the
        // manifest but mark it unavailable rather than waiting for a paint.
        let native_viewports =
            std::collections::HashSet::from([egui::ViewportId::ROOT, second.viewport_id()]);
        let targets = runtime
            .capture_targets(
                CaptureScope::All,
                &TestScriptActionSelection::LegacyImplicit,
                &native_viewports,
            )
            .unwrap();
        assert_eq!(targets[1].availability, capture::Availability::Absent);
        assert_eq!(targets[2].availability, capture::Availability::Registered);

        // A parked still host may outlive its context binding. The backend
        // style witness keeps it in capture("all") even without a window snapshot.
        let parked_id = egui::ViewportId::from_hash_of(("test-window", 3_u64));
        runtime
            .snapshot
            .write()
            .unwrap()
            .host_styles
            .push(TestScriptHostStyle {
                role: "detached".into(),
                window_id: Some(3),
                presentation: TestScriptWindowPresentation::PassiveDeferredFrozen,
                viewport: format!("{parked_id:?}"),
                viewport_id: Some(parked_id),
                hwnd: 0x3030,
                backend_token: Some(3),
                topmost: true,
                noactivate: false,
                minimized: false,
                visible: true,
                owner_hwnd: 0,
                foreground_hwnd: 0,
            });
        let native_viewports = std::collections::HashSet::from([
            egui::ViewportId::ROOT,
            first.viewport_id(),
            second.viewport_id(),
            parked_id,
        ]);
        let targets = runtime
            .capture_targets(
                CaptureScope::All,
                &TestScriptActionSelection::LegacyImplicit,
                &native_viewports,
            )
            .unwrap();
        assert_eq!(targets.len(), 4);
        assert_eq!(targets[3].role, "detached-3");
        assert_eq!(
            targets[3].presentation,
            TestScriptWindowPresentation::PassiveDeferredFrozen
        );
    }

    #[test]
    fn fresh_owner_barrier_accepts_the_exact_live_ui_and_backend_identity() {
        let context = egui::Context::default();
        let viewport = egui::ViewportId::from_hash_of("fresh-owner-current");
        let fixture = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let witness = {
            let _scope = fixture.enter(&context, viewport, 0x500);
            eframe::miv_test_script_window_witness::active().unwrap()
        };
        let owner = detached_identity_from_witness(witness, 11, 13);
        let mut snapshot = ready_snapshot();
        snapshot.windows = vec![window_snapshot(owner.clone(), 17, 0, "video::current")];
        let (bridge, rx, _) = runner_bridge(snapshot);
        *bridge.action_selection.lock().unwrap() =
            TestScriptActionSelection::Targeted(owner.clone());
        let worker = {
            let bridge = bridge.clone();
            let owner = owner.clone();
            std::thread::spawn(move || {
                bridge
                    .validate_selected_owner_fresh(&owner, Instant::now() + Duration::from_secs(1))
            })
        };
        let mut runtime = local_runtime();
        runtime
            .publish_windows(vec![window_snapshot(
                owner.clone(),
                17,
                0,
                "video::current",
            )])
            .unwrap();
        match rx.recv_timeout(Duration::from_secs(1)).unwrap() {
            UiCommand::ValidateSelectedOwner {
                expected_identity,
                reply,
            } => {
                assert_eq!(expected_identity, owner);
                reply
                    .send(runtime.validate_selected_owner(&expected_identity))
                    .unwrap();
            }
            command => panic!("unexpected command: {command:?}"),
        }
        worker.join().unwrap().unwrap();
    }

    #[test]
    fn fresh_owner_barrier_rejects_a_logical_replacement_despite_a_cached_snapshot() {
        let context = egui::Context::default();
        let viewport = egui::ViewportId::from_hash_of("fresh-owner-replaced");
        let fixture = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let witness = {
            let _scope = fixture.enter(&context, viewport, 0x501);
            eframe::miv_test_script_window_witness::active().unwrap()
        };
        let old_owner = detached_identity_from_witness(witness, 11, 13);
        let replacement = detached_identity_from_witness(witness, 12, 14);
        let mut cached = ready_snapshot();
        cached.windows = vec![window_snapshot(old_owner.clone(), 17, 0, "video::old")];
        let (bridge, rx, _) = runner_bridge(cached);
        *bridge.action_selection.lock().unwrap() =
            TestScriptActionSelection::Targeted(old_owner.clone());
        let worker = {
            let bridge = bridge.clone();
            let old_owner = old_owner.clone();
            std::thread::spawn(move || {
                bridge.validate_selected_owner_fresh(
                    &old_owner,
                    Instant::now() + Duration::from_secs(1),
                )
            })
        };
        let mut runtime = local_runtime();
        runtime
            .publish_windows(vec![window_snapshot(
                replacement,
                18,
                0,
                "video::replacement",
            )])
            .unwrap();
        match rx.recv_timeout(Duration::from_secs(1)).unwrap() {
            UiCommand::ValidateSelectedOwner {
                expected_identity,
                reply,
            } => reply
                .send(runtime.validate_selected_owner(&expected_identity))
                .unwrap(),
            command => panic!("unexpected command: {command:?}"),
        }
        let error = worker.join().unwrap().unwrap_err();
        assert!(error.contains("no longer authoritative"));
    }

    #[test]
    fn owner_barrier_disconnect_is_an_environment_failure() {
        let context = egui::Context::default();
        let viewport = egui::ViewportId::from_hash_of("fresh-owner-disconnect");
        let fixture = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let witness = {
            let _scope = fixture.enter(&context, viewport, 0x502);
            eframe::miv_test_script_window_witness::active().unwrap()
        };
        let owner = detached_identity_from_witness(witness, 11, 13);
        let mut snapshot = ready_snapshot();
        snapshot.windows = vec![window_snapshot(owner.clone(), 17, 0, "video::current")];
        let (bridge, rx, _) = runner_bridge(snapshot);
        *bridge.action_selection.lock().unwrap() =
            TestScriptActionSelection::Targeted(owner.clone());
        drop(rx);

        let error = bridge
            .validate_selected_owner_fresh(&owner, Instant::now() + Duration::from_secs(1))
            .unwrap_err();
        assert!(error.contains("disconnected before dispatch"));
        assert_eq!(
            bridge.interrupt.failure_message().as_deref(),
            Some(error.as_str())
        );
    }

    #[test]
    fn owner_barrier_rejects_finishing_or_cancelled_ui_runtime() {
        let context = egui::Context::default();
        let fixture = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let witness = {
            let _scope = fixture.enter(&context, egui::ViewportId::ROOT, 0x503);
            eframe::miv_test_script_window_witness::active().unwrap()
        };
        let owner = TestScriptWindowIdentity::Root {
            context_serial: 11,
            hwnd: witness.hwnd(),
            backend_token: witness.token(),
        };
        let mut runtime = local_runtime();
        runtime
            .publish_windows(vec![window_snapshot(owner.clone(), 17, 0, "root::current")])
            .unwrap();
        runtime.cancel_requested = true;
        assert!(
            runtime
                .validate_selected_owner(&owner)
                .unwrap_err()
                .contains("cancellation")
        );
        runtime.cancel_requested = false;
        runtime.begin_finish(ScriptOutcome::success(), 1);
        assert!(
            runtime
                .validate_selected_owner(&owner)
                .unwrap_err()
                .contains("already finishing")
        );
    }

    #[test]
    fn targeted_action_waits_for_exact_owner_and_does_not_repeat_after_peek() {
        let owner = window_identity(7, 11, 14);
        let sibling = window_identity(8, 12, 15);
        let action = KeyAction::FsPageNext;
        let mut runtime = local_runtime();
        runtime
            .publish_windows(vec![
                window_snapshot(owner.clone(), 17, 2, "pdf::owner#2"),
                window_snapshot(sibling.clone(), 18, 4, "pdf::sibling#4"),
            ])
            .unwrap();
        let (applied, acknowledgement) = mpsc::sync_channel(1);

        assert_eq!(
            runtime.queue_action(
                action,
                TestScriptActionSelection::Targeted(owner.clone()),
                applied,
                false,
            ),
            None,
            "detached owner is resolved by App before a pass is eligible"
        );
        assert_eq!(
            runtime.pending_targeted_detached_owner(),
            Some(owner.clone())
        );
        assert!(!consume_pending_action_from(
            &mut runtime.pending_actions,
            Some(&owner),
            action
        ));
        runtime.finish_targeted_detached_owner(&owner, Ok(()));
        assert!(!peek_pending_action_from(
            &mut runtime.pending_actions,
            Some(&owner),
            action
        ));
        runtime.promote_focused_action_targets(|candidate| candidate == &sibling);
        assert!(!peek_pending_action_from(
            &mut runtime.pending_actions,
            Some(&owner),
            action
        ));
        runtime.promote_focused_action_targets(|candidate| candidate == &owner);
        assert!(!peek_pending_action_from(
            &mut runtime.pending_actions,
            Some(&sibling),
            action
        ));
        runtime.finish_target_pass(&sibling, true, 1);
        runtime.finish_target_pass(&owner, false, 1);
        assert!(matches!(
            acknowledgement.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));

        assert!(peek_pending_action_from(
            &mut runtime.pending_actions,
            Some(&owner),
            action
        ));
        assert_eq!(
            acknowledgement
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            Ok(())
        );
        assert!(peek_pending_action_from(
            &mut runtime.pending_actions,
            Some(&owner),
            action
        ));
        runtime.finish_target_pass(&owner, false, 1);
        assert!(runtime.pending_actions.is_empty());
        assert!(!peek_pending_action_from(
            &mut runtime.pending_actions,
            Some(&owner),
            action
        ));
    }

    #[test]
    fn fullscreen_open_action_never_queues_source_focus_in_its_dispatch_pass() {
        let owner = root_identity(11, 0x503);
        let action = KeyAction::GridOpenSelected;
        let mut runtime = local_runtime();
        runtime
            .publish_windows(vec![window_snapshot(
                owner.clone(),
                17,
                0,
                "still::selected",
            )])
            .unwrap();

        // An already focused root dispatches immediately without a Focus command.
        let (applied, acknowledged) = mpsc::sync_channel(1);
        assert_eq!(
            runtime.queue_action(
                action,
                TestScriptActionSelection::Targeted(owner.clone()),
                applied,
                true,
            ),
            None
        );
        assert!(consume_pending_action_from(
            &mut runtime.pending_actions,
            Some(&owner),
            action
        ));
        assert_eq!(acknowledged.try_recv().unwrap(), Ok(()));

        // If the root is unfocused, its Focus belongs to an earlier pass.
        let (applied, acknowledged) = mpsc::sync_channel(1);
        assert_eq!(
            runtime.queue_action(
                action,
                TestScriptActionSelection::Targeted(owner.clone()),
                applied,
                false,
            ),
            Some(egui::ViewportId::ROOT)
        );
        assert!(!consume_pending_action_from(
            &mut runtime.pending_actions,
            Some(&owner),
            action
        ));
        runtime.promote_focused_action_targets(|_| false);
        assert!(!consume_pending_action_from(
            &mut runtime.pending_actions,
            Some(&owner),
            action
        ));
        runtime.promote_focused_action_targets(|candidate| candidate == &owner);
        assert!(consume_pending_action_from(
            &mut runtime.pending_actions,
            Some(&owner),
            action
        ));
        assert_eq!(acknowledged.try_recv().unwrap(), Ok(()));
    }

    #[test]
    fn root_frame_expiry_does_not_expire_a_detached_target_before_its_pass() {
        let owner = window_identity(7, 11, 14);
        let action = KeyAction::FsClose;
        let mut runtime = local_runtime();
        runtime
            .publish_windows(vec![window_snapshot(owner.clone(), 17, 2, "pdf::owner#2")])
            .unwrap();
        let (applied, acknowledgement) = mpsc::sync_channel(1);
        runtime.queue_action(
            action,
            TestScriptActionSelection::Targeted(owner.clone()),
            applied,
            false,
        );

        runtime.expire_unconsumed_legacy_actions(2);
        assert_eq!(runtime.pending_actions.len(), 1);
        assert!(matches!(
            acknowledgement.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        runtime.finish_targeted_detached_owner(&owner, Ok(()));
        runtime.promote_focused_action_targets(|candidate| candidate == &owner);
        runtime.finish_target_pass(&owner, true, 2);
        let error = acknowledgement
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap_err();
        assert!(error.contains("target UI pass"));
    }

    #[test]
    fn stale_target_is_rejected_without_legacy_fallback() {
        let owner = window_identity(7, 11, 14);
        let sibling = window_identity(8, 12, 15);
        let action = KeyAction::FsClose;
        let mut runtime = local_runtime();
        runtime
            .publish_windows(vec![window_snapshot(owner.clone(), 17, 2, "pdf::owner#2")])
            .unwrap();
        let (applied, acknowledgement) = mpsc::sync_channel(1);
        runtime.queue_action(
            action,
            TestScriptActionSelection::Targeted(owner.clone()),
            applied,
            false,
        );

        runtime
            .publish_windows(vec![window_snapshot(
                sibling.clone(),
                18,
                4,
                "pdf::sibling#4",
            )])
            .unwrap();
        let error = acknowledgement
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap_err();
        assert!(error.contains("no longer current"));
        assert!(!consume_pending_action_from(
            &mut runtime.pending_actions,
            Some(&sibling),
            action
        ));
    }

    #[test]
    fn backend_replacement_stales_target_without_a_manager_claim_change() {
        let old_owner = window_identity_with_backend_token(7, 11, 14, 101);
        let replacement_owner = window_identity_with_backend_token(7, 11, 14, 102);
        let action = KeyAction::FsClose;
        let mut runtime = local_runtime();
        runtime
            .publish_windows(vec![window_snapshot(
                old_owner.clone(),
                17,
                2,
                "pdf::owner#2",
            )])
            .unwrap();
        let (applied, acknowledgement) = mpsc::sync_channel(1);
        runtime.queue_action(
            action,
            TestScriptActionSelection::Targeted(old_owner),
            applied,
            false,
        );

        runtime
            .publish_windows(vec![window_snapshot(
                replacement_owner,
                17,
                2,
                "pdf::owner#2",
            )])
            .unwrap();

        let error = acknowledgement
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap_err();
        assert!(error.contains("no longer current"));
        assert!(runtime.pending_actions.is_empty());
    }

    #[test]
    fn legacy_action_retains_first_matching_consumer_behavior() {
        let action = KeyAction::GridMoveFirst;
        let owner = window_identity(7, 11, 14);
        let mut runtime = local_runtime();
        let (applied, acknowledgement) = mpsc::sync_channel(1);
        runtime.queue_action(
            action,
            TestScriptActionSelection::LegacyImplicit,
            applied,
            true,
        );

        assert!(consume_pending_action_from(
            &mut runtime.pending_actions,
            Some(&owner),
            action
        ));
        assert_eq!(
            acknowledgement
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            Ok(())
        );
    }

    #[test]
    fn action_pass_observations_are_partitioned_by_viewport_at_the_same_pass() {
        let ctx = egui::Context::default();
        let root = root_identity(1, 0x100);
        let child = window_identity(7, 11, 14);
        let pass = 9;
        let root_observation = TestScriptActionPassObservation {
            pass,
            owner: Some(root.clone()),
            eligible: true,
        };
        let child_observation = TestScriptActionPassObservation {
            pass,
            owner: Some(child.clone()),
            eligible: false,
        };
        let child_viewport = child.viewport_id();
        ctx.data_mut(|data| {
            data.insert_temp(
                action_pass_observation_id(egui::ViewportId::ROOT),
                root_observation,
            );
            data.insert_temp(
                action_pass_observation_id(child_viewport),
                child_observation,
            );
        });

        ctx.data(|data| {
            assert_eq!(
                data.get_temp::<TestScriptActionPassObservation>(action_pass_observation_id(
                    egui::ViewportId::ROOT
                ))
                .and_then(|observation| observation.owner),
                Some(root)
            );
            assert_eq!(
                data.get_temp::<TestScriptActionPassObservation>(action_pass_observation_id(
                    child_viewport
                ))
                .and_then(|observation| observation.owner),
                Some(child)
            );
        });
    }

    #[test]
    fn action_pass_publication_and_eligibility_use_egui_data_without_reentry() {
        let ctx = egui::Context::default();
        let fixture = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let _scope = fixture.enter(&ctx, egui::ViewportId::ROOT, 0x100);
        let witness = eframe::miv_test_script_window_witness::active().unwrap();
        let owner = TestScriptWindowIdentity::Root {
            context_serial: 1,
            hwnd: 0x100,
            backend_token: witness.token(),
        };
        ctx.begin_pass(Default::default());

        publish_action_pass_owner(&ctx, Some(owner.clone()));
        mark_action_pass_eligible(&ctx);
        let observation = action_pass_observation(&ctx).expect("current pass observation");

        assert_eq!(observation.owner, Some(owner));
        assert!(observation.eligible);
        let _ = ctx.end_pass();
    }

    #[test]
    fn replacement_backend_with_the_same_hwnd_cannot_consume_an_old_target() {
        let ctx = egui::Context::default();
        let first = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let old_owner = {
            let _scope = first.enter(&ctx, egui::ViewportId::ROOT, 0x101);
            TestScriptWindowIdentity::Root {
                context_serial: 1,
                hwnd: 0x101,
                backend_token: eframe::miv_test_script_window_witness::active()
                    .unwrap()
                    .token(),
            }
        };
        let replacement = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let _scope = replacement.enter(&ctx, egui::ViewportId::ROOT, 0x101);
        ctx.begin_pass(Default::default());

        publish_action_pass_owner(&ctx, Some(old_owner.clone()));
        let observed_owner = action_pass_observation_for_active_backend(&ctx)
            .and_then(|observation| observation.owner);
        let (applied, _acknowledgement) = mpsc::sync_channel(1);
        let mut pending = VecDeque::from([PendingAction {
            action: KeyAction::GridToggleDetailsView,
            dispatch: PendingActionDispatch::Targeted {
                owner: old_owner,
                phase: TargetedActionPhase::AwaitingPass,
            },
            applied: Some(applied),
        }]);

        assert_eq!(observed_owner, None);
        assert!(!consume_pending_action_from(
            &mut pending,
            observed_owner.as_ref(),
            KeyAction::GridToggleDetailsView
        ));
        assert_eq!(pending.len(), 1);
        let _ = ctx.end_pass();
    }

    #[test]
    fn backend_witness_scope_is_context_scoped_nested_and_thread_local() {
        let first_context = egui::Context::default();
        let second_context = egui::Context::default();
        let viewport = egui::ViewportId::from_hash_of("same-viewport-separate-contexts");
        let first = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let second = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let first_scope = first.enter(&first_context, viewport, 0x201);
        let first_witness = eframe::miv_test_script_window_witness::active().unwrap();

        assert_eq!(
            eframe::miv_test_script_window_witness::latest(viewport),
            Some(first_witness)
        );
        std::thread::spawn(move || {
            assert_eq!(eframe::miv_test_script_window_witness::active(), None);
            assert_eq!(
                eframe::miv_test_script_window_witness::latest(viewport),
                None
            );
        })
        .join()
        .unwrap();
        {
            let _second_scope = second.enter(&second_context, viewport, 0x202);
            let second_witness = eframe::miv_test_script_window_witness::active().unwrap();
            assert_ne!(first_witness.token(), second_witness.token());
            assert_eq!(
                eframe::miv_test_script_window_witness::latest(viewport),
                Some(second_witness)
            );
        }

        assert_eq!(
            eframe::miv_test_script_window_witness::active(),
            Some(first_witness)
        );
        assert_eq!(
            eframe::miv_test_script_window_witness::latest(viewport),
            Some(first_witness)
        );
        drop(first_scope);
        assert_eq!(eframe::miv_test_script_window_witness::active(), None);
    }

    fn content_proof(
        context_serial: u64,
        generation: u64,
        page_index: usize,
        item: &str,
        texture: u64,
        source_kind: TestScriptPaintSourceKind,
    ) -> TestScriptContentProof {
        TestScriptContentProof {
            context_serial,
            items_generation: generation,
            page_index,
            item_identity: item.to_string(),
            source_texture_id: egui::TextureId::Managed(texture),
            source_kind,
            final_composite_complete: false,
        }
    }

    fn window_snapshot(
        identity: TestScriptWindowIdentity,
        generation: u64,
        page_index: usize,
        item: &str,
    ) -> TestScriptWindowSnapshot {
        TestScriptWindowSnapshot {
            identity: Some(identity.clone()),
            role: identity.role().to_string(),
            window_id: identity.window_id(),
            context_serial: identity.context_serial(),
            viewport_id: identity.viewport_id(),
            host_incarnation: identity.host_incarnation(),
            hwnd: Some(identity.hwnd()),
            backend_token: Some(identity.backend_token()),
            residence: "at_rest".to_string(),
            presentation: if identity.window_id().is_some() {
                TestScriptWindowPresentation::ActiveImmediate
            } else {
                TestScriptWindowPresentation::Root
            },
            media_kind: "pdf".to_string(),
            page_index: Some(page_index),
            items_generation: generation,
            item_identity: item.to_string(),
            selected_item_identity: String::new(),
            page_ready: true,
            viewport_rendered: false,
            viewport_revision: 0,
            paint_matches_current_page: false,
            full_texture_painted: false,
            paint_source: String::new(),
            paint_source_texture: String::new(),
            painted_page_index: None,
            paint_revision: 0,
            paints: Vec::new(),
            sidecar_imported: false,
            sidecar_loaded: false,
            seek_strip: TestScriptSeekStripSnapshot::closed(),
        }
    }

    #[test]
    fn paint_evidence_requires_the_exact_owner_and_current_page_identity() {
        let owner = window_identity(7, 11, 13);
        let current = window_snapshot(owner.clone(), 17, 2, "pdf::current#2");
        let exact = content_proof(
            11,
            17,
            2,
            "pdf::current#2",
            19,
            TestScriptPaintSourceKind::FullOrProcessed,
        );
        let mut observations = HashMap::new();
        observations.insert(
            TestScriptPaintEvidenceKey {
                owner: owner.clone(),
                content: exact.clone(),
            },
            1,
        );
        assert!(
            joined_window_snapshots(
                &[current.clone()],
                &HashMap::new(),
                &observations,
                &HashMap::new(),
                &Default::default()
            )[0]
            .paint_matches_current_page
        );

        let stale_cases = [
            TestScriptPaintEvidenceKey {
                owner: window_identity(8, 11, 13),
                content: exact.clone(),
            },
            TestScriptPaintEvidenceKey {
                owner: window_identity(7, 12, 13),
                content: content_proof(
                    12,
                    17,
                    2,
                    "pdf::current#2",
                    19,
                    TestScriptPaintSourceKind::FullOrProcessed,
                ),
            },
            TestScriptPaintEvidenceKey {
                owner: window_identity(7, 11, 14),
                content: exact.clone(),
            },
            TestScriptPaintEvidenceKey {
                owner: owner.clone(),
                content: content_proof(
                    11,
                    18,
                    2,
                    "pdf::current#2",
                    19,
                    TestScriptPaintSourceKind::FullOrProcessed,
                ),
            },
            TestScriptPaintEvidenceKey {
                owner: owner.clone(),
                content: content_proof(
                    11,
                    17,
                    3,
                    "pdf::current#2",
                    19,
                    TestScriptPaintSourceKind::FullOrProcessed,
                ),
            },
            TestScriptPaintEvidenceKey {
                owner,
                content: content_proof(
                    11,
                    17,
                    2,
                    "pdf::other#2",
                    19,
                    TestScriptPaintSourceKind::FullOrProcessed,
                ),
            },
        ];
        for stale in stale_cases {
            let joined = joined_window_snapshots(
                &[current.clone()],
                &HashMap::new(),
                &HashMap::from([(stale, 2)]),
                &HashMap::new(),
                &Default::default(),
            );
            assert!(!joined[0].paint_matches_current_page);
        }
    }

    #[test]
    fn late_old_callback_cannot_replace_current_full_paint_evidence() {
        let current_owner = window_identity(7, 11, 14);
        let old_owner = window_identity(7, 11, 13);
        let window = window_snapshot(current_owner.clone(), 17, 2, "pdf::current#2");
        let full = content_proof(
            11,
            17,
            2,
            "pdf::current#2",
            20,
            TestScriptPaintSourceKind::FullOrProcessed,
        );
        let thumbnail = content_proof(
            11,
            17,
            2,
            "pdf::current#2",
            19,
            TestScriptPaintSourceKind::CatalogThumbnail,
        );
        let observations = HashMap::from([
            (
                TestScriptPaintEvidenceKey {
                    owner: current_owner.clone(),
                    content: full,
                },
                2,
            ),
            (
                TestScriptPaintEvidenceKey {
                    owner: old_owner,
                    content: thumbnail.clone(),
                },
                3,
            ),
            (
                TestScriptPaintEvidenceKey {
                    owner: current_owner,
                    content: thumbnail,
                },
                4,
            ),
        ]);

        let joined = joined_window_snapshots(
            &[window],
            &HashMap::new(),
            &observations,
            &HashMap::new(),
            &Default::default(),
        );
        assert!(joined[0].full_texture_painted);
        assert_eq!(joined[0].paint_revision, 2);
        assert_eq!(joined[0].paint_source, "full_or_processed");
    }

    #[test]
    fn repeated_textures_keep_one_best_paint_observation_per_owner() {
        let (_tx, rx) = mpsc::channel();
        let snapshot = Arc::new(RwLock::new(TestScriptSnapshot::default()));
        let mut runtime = UiRuntime::new(
            rx,
            Arc::clone(&snapshot),
            Arc::new(InterruptState::default()),
            pointer_input::new_shared_catalog(),
        );
        let owner = window_identity(7, 11, 14);
        runtime
            .publish_windows(vec![window_snapshot(
                owner.clone(),
                17,
                2,
                "pdf::current#2",
            )])
            .unwrap();

        for texture in 100..164 {
            runtime
                .publish_window_frame(
                    owner.clone(),
                    Some(content_proof(
                        11,
                        17,
                        2,
                        "pdf::current#2",
                        texture,
                        TestScriptPaintSourceKind::CatalogThumbnail,
                    )),
                )
                .unwrap();
        }
        assert_eq!(runtime.paint_observations.len(), 1);

        runtime
            .publish_window_frame(
                owner.clone(),
                Some(content_proof(
                    11,
                    17,
                    2,
                    "pdf::current#2",
                    1000,
                    TestScriptPaintSourceKind::FullOrProcessed,
                )),
            )
            .unwrap();
        let full_revision = snapshot.read().unwrap().windows[0].paint_revision;
        runtime
            .publish_window_frame(
                owner,
                Some(content_proof(
                    11,
                    17,
                    2,
                    "pdf::current#2",
                    1001,
                    TestScriptPaintSourceKind::CatalogThumbnail,
                )),
            )
            .unwrap();

        assert_eq!(runtime.paint_observations.len(), 1);
        let published = snapshot.read().unwrap();
        assert!(published.windows[0].full_texture_painted);
        assert_eq!(published.windows[0].paint_revision, full_revision);
        assert!(published.windows[0].viewport_revision > full_revision);
    }

    #[test]
    fn table_change_prunes_stale_callbacks_without_clearing_current_evidence() {
        let (_tx, rx) = mpsc::channel();
        let snapshot = Arc::new(RwLock::new(TestScriptSnapshot::default()));
        let mut runtime = UiRuntime::new(
            rx,
            Arc::clone(&snapshot),
            Arc::new(InterruptState::default()),
            pointer_input::new_shared_catalog(),
        );
        let old_owner = window_identity(7, 11, 13);
        let current_owner = window_identity(7, 11, 14);
        let content = content_proof(
            11,
            17,
            2,
            "pdf::current#2",
            20,
            TestScriptPaintSourceKind::FullOrProcessed,
        );
        runtime
            .publish_windows(vec![window_snapshot(
                old_owner.clone(),
                17,
                2,
                "pdf::current#2",
            )])
            .unwrap();
        assert!(
            runtime
                .publish_window_frame(old_owner.clone(), Some(content.clone()))
                .unwrap()
        );

        runtime
            .publish_windows(vec![window_snapshot(
                current_owner.clone(),
                17,
                2,
                "pdf::current#2",
            )])
            .unwrap();
        assert!(
            runtime
                .publish_window_frame(current_owner, Some(content.clone()))
                .unwrap()
        );
        assert!(
            !runtime
                .publish_window_frame(old_owner, Some(content))
                .unwrap()
        );
        let published = snapshot.read().unwrap();
        assert!(published.windows[0].paint_matches_current_page);
        assert!(published.windows[0].full_texture_painted);
    }

    #[test]
    fn outcome_kinds_map_to_process_exit_codes() {
        assert_eq!(ScriptOutcomeKind::Success.exit_code(), 0);
        assert_ne!(ScriptOutcomeKind::ScriptFailure.exit_code(), 0);
        assert_ne!(ScriptOutcomeKind::EnvironmentFailure.exit_code(), 0);
    }

    #[cfg(windows)]
    #[test]
    fn poisoned_pointer_catalog_fails_the_actual_show_and_releases_its_exact_step() {
        let _serial = crate::key_input::lock_test_input();

        let ctx = egui::Context::default();
        let viewport = egui::ViewportId::from_hash_of("poisoned-pointer-catalog");
        let backend = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let _backend_scope = backend.enter(&ctx, viewport, 0x7272);
        let witness = eframe::miv_test_script_window_witness::active().unwrap();
        let owner = detached_identity_from_witness(witness, 17, 19);
        let pointer_owner = crate::key_input::SyntheticPointerOwner {
            identity: owner.clone(),
            items_generation: 29,
        };
        let mode = pointer_input::FullscreenModeProof {
            spread_mode: "Single".to_string(),
            reading_flow: "Paged".to_string(),
            strip_rtl: false,
            seek_bar_rtl: false,
            strip_visible: true,
            strip_locked: true,
            bar_locked: true,
        };
        let mode_signature = crate::key_input::SyntheticPointerModeSignature {
            spread_mode: mode.spread_mode.clone(),
            reading_flow: mode.reading_flow.clone(),
            strip_rtl: mode.strip_rtl,
            seek_bar_rtl: mode.seek_bar_rtl,
            strip_visible: mode.strip_visible,
            strip_locked: mode.strip_locked,
            bar_locked: mode.bar_locked,
        };
        let rect = egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(110.0, 40.0));
        let point = egui::pos2(20.0, 30.0);
        let prepared = crate::key_input::prepared_synthetic_pointer_step_for_test(
            73,
            crate::key_input::SyntheticPointerLatch {
                transaction_id: 73,
                owner: pointer_owner,
                region: crate::key_input::SyntheticPointerRegion::StillSeekTrack,
                mode: mode_signature,
                region_geometry_token: 1,
                press_page_index: 2,
                press_item_identity: "page-2".to_string(),
                widget_id: egui::Id::new("poisoned-pointer-track"),
                press_rect: rect,
                coordinate_frame: rect,
                press_pixels_per_point: 1.0,
                press_point: point,
            },
            crate::key_input::SyntheticPointerStepKind::Down { point },
            1,
            1.0_f64.to_bits(),
        );
        let completion =
            crate::key_input::install_synthetic_pointer_delivered_for_test(prepared.clone());

        let mut warmup_input = egui::RawInput {
            viewport_id: viewport,
            time: Some(1.0),
            ..Default::default()
        };
        warmup_input.viewports.insert(
            viewport,
            egui::ViewportInfo {
                parent: Some(egui::ViewportId::ROOT),
                native_pixels_per_point: Some(1.0),
                ..Default::default()
            },
        );
        ctx.begin_pass(warmup_input);
        egui::CentralPanel::default().show(&ctx, |ui| {
            let _ = ui.interact(
                rect,
                egui::Id::new("poisoned-pointer-track"),
                egui::Sense::click_and_drag(),
            );
        });
        let _ = ctx.end_pass();

        let mut child_input = egui::RawInput {
            viewport_id: viewport,
            time: Some(1.005),
            events: vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            ..Default::default()
        };
        child_input.viewports.insert(
            viewport,
            egui::ViewportInfo {
                parent: Some(egui::ViewportId::ROOT),
                native_pixels_per_point: Some(1.0),
                ..Default::default()
            },
        );
        let show = pointer_input::enter_show(Some(pointer_input::ShowOwner {
            identity: owner.clone(),
            items_generation: 29,
        }));
        ctx.begin_pass(child_input.clone());
        pointer_input::record_delivery_proof(&prepared, &child_input);
        pointer_input::begin_pass(&ctx, 29, 2, "page-2".to_string(), mode);
        egui::CentralPanel::default().show(&ctx, |ui| {
            let response = ui.interact(
                rect,
                egui::Id::new("poisoned-pointer-track"),
                egui::Sense::click_and_drag(),
            );
            assert!(
                response.is_pointer_button_down_on(),
                "test precondition: egui must hit the actual track response"
            );
            pointer_input::record_region(
                pointer_input::RegionId::StillSeekTrack,
                &response,
                rect,
                None,
            );
            pointer_input::observe_region_handler(
                &response,
                pointer_input::RegionId::StillSeekTrack,
                None,
            );
        });
        pointer_input::finish_pass(&ctx);
        let output = show.finish();
        let _ = ctx.end_pass();

        let poisoned_catalog = pointer_input::new_shared_catalog();
        let poison_target = Arc::clone(&poisoned_catalog);
        let poison = std::thread::spawn(move || {
            let _guard = poison_target.write().unwrap();
            panic!("poison pointer catalog for publication regression");
        });
        assert!(poison.join().is_err());
        let mut ui_runtime = local_runtime();
        ui_runtime.pointer_regions = poisoned_catalog;
        {
            let mut active = runtime().lock().expect("test-script runtime lock poisoned");
            assert!(active.is_none(), "no other runtime may own this regression");
            *active = Some(ui_runtime);
        }

        publish_pointer_show(&ctx, output, owner, 29, 2);

        let completion_error = completion
            .recv_timeout(Duration::from_secs(1))
            .expect("the exact delivered step must be completed")
            .expect_err("catalog publication failure must not report pointer success");
        assert!(completion_error.contains("catalog is poisoned"));
        assert!(crate::key_input::synthetic_input_is_idle());
        let mut active = runtime().lock().expect("test-script runtime lock poisoned");
        let mut ui_runtime = active
            .take()
            .expect("the regression runtime must remain present");
        ui_runtime.begin_finish(ScriptOutcome::success(), 1.005_f64.to_bits());
        let finish = ui_runtime
            .finish
            .expect("environment failure must finish the run");
        assert_eq!(finish.outcome.kind, ScriptOutcomeKind::EnvironmentFailure);
        assert_ne!(finish.outcome.kind.exit_code(), 0);
        assert!(finish.outcome.message.contains("catalog is poisoned"));
        drop(active);
        crate::key_input::clear_test_synthetic_input();
    }

    #[cfg(windows)]
    #[test]
    fn pointer_terminal_ack_cannot_be_masked_by_same_frame_success() {
        let _serial = crate::key_input::lock_test_input();
        let handle = crate::key_input::SyntheticPointerCancelHandle {
            transaction_id: 73,
            step_id: 73_u64 << 32 | 1,
            owner: crate::key_input::SyntheticPointerOwner {
                identity: root_identity(17, 0x7171),
                items_generation: 29,
            },
        };
        crate::key_input::install_synthetic_pointer_terminal_for_test(handle.clone());
        let mut runtime = local_runtime();

        runtime.fail_environment(
            "synthetic pointer cleanup did not release primary".to_string(),
            41,
        );
        assert!(crate::key_input::acknowledge_synthetic_pointer_terminal_issue(&handle));
        runtime.begin_finish(ScriptOutcome::success(), 41);

        let finish = runtime.finish.as_ref().expect("finish must be committed");
        assert_eq!(finish.outcome.kind, ScriptOutcomeKind::EnvironmentFailure);
        assert_ne!(finish.outcome.kind.exit_code(), 0);
        assert!(crate::key_input::synthetic_input_is_idle());
        crate::key_input::clear_test_synthetic_input();
    }
}
