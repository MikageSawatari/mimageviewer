//! Root-window OLE target. Callbacks own snapshots, never borrow App.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::shell_file_ops::{ShellTransferOperation, ShellTransferRequest};

pub(crate) type SharedMainDrop = Rc<RefCell<MainDropShared>>;

pub(crate) struct MainDropShared {
    pub(crate) geometries: VecDeque<(u64, DropGeometry)>,
    pub(crate) admission: Result<(), String>,
    pub(crate) internal: Option<InternalDrag>,
    pub(crate) drops: Vec<DropRequest>,
}

impl Default for MainDropShared {
    fn default() -> Self {
        Self {
            geometries: VecDeque::new(),
            admission: Err("ドロップの準備ができていません".into()),
            internal: None,
            drops: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct InternalDrag {
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) excluded_virtual: bool,
    pub(crate) failed_paths: usize,
}

#[derive(Debug)]
pub(crate) enum DropRequest {
    GridFiles(Vec<PathBuf>),
    TreeTransfer(ShellTransferRequest),
    Failed(String),
}

#[derive(Clone, Debug)]
pub(crate) struct DropRow {
    pub(crate) rect: egui::Rect,
    pub(crate) path: PathBuf,
}

#[derive(Clone, Debug)]
pub(crate) struct DropGeometry {
    pub(crate) pane_rect: egui::Rect,
    pub(crate) tree_clip: egui::Rect,
    pub(crate) rows: Vec<DropRow>,
    pub(crate) client_size_px: [i32; 2],
    pub(crate) dpi: u32,
    pub(crate) pixels_per_point: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WindowMetrics {
    pub(crate) client_size_px: [i32; 2],
    pub(crate) dpi: u32,
}

pub(crate) fn publish_geometry(
    shared: &SharedMainDrop,
    ctx: &egui::Context,
    geometry: DropGeometry,
) {
    let presented = egui_wgpu::last_presented_pass(ctx, egui::ViewportId::ROOT);
    let pass = ctx.cumulative_pass_nr() + 1;
    let mut shared = shared.borrow_mut();
    if let Some(presented) = presented {
        shared.geometries.retain(|(pass, _)| *pass >= presented);
    }
    shared.geometries.retain(|(old, _)| *old != pass);
    shared.geometries.push_back((pass, geometry));
    // Keep the displayed geometry even during many consecutive skipped presents.
    // Only the newest few unpublished passes can subsequently be presented.
    while shared.geometries.len() > 4 {
        let remove = usize::from(
            shared
                .geometries
                .front()
                .is_some_and(|(pass, _)| Some(*pass) == presented),
        );
        let _ = shared.geometries.remove(remove);
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Hit {
    TreeRow(PathBuf),
    TreeNonTarget,
    ExistingDropArea,
}

fn hit_test(
    geometries: &VecDeque<(u64, DropGeometry)>,
    presented: Option<u64>,
    metrics: WindowMetrics,
    client_px: [i32; 2],
) -> Option<Hit> {
    let pass = presented?;
    let geometry = &geometries.iter().find(|(nr, _)| *nr == pass)?.1;
    if geometry.client_size_px != metrics.client_size_px
        || geometry.dpi != metrics.dpi
        || !geometry.pixels_per_point.is_finite()
        || geometry.pixels_per_point <= 0.0
    {
        return None;
    }
    let point = egui::pos2(
        client_px[0] as f32 / geometry.pixels_per_point,
        client_px[1] as f32 / geometry.pixels_per_point,
    );
    if !geometry.pane_rect.contains(point) {
        return Some(Hit::ExistingDropArea);
    }
    if geometry.tree_clip.contains(point)
        && let Some(row) = geometry.rows.iter().find(|row| row.rect.contains(point))
    {
        return Some(Hit::TreeRow(row.path.clone()));
    }
    Some(Hit::TreeNonTarget)
}

/// Numeric OLE modifiers, passed by the drag loop (never sampled from live keys).
const SHIFT: u32 = 4;
const CONTROL: u32 = 8;
const ALT: u32 = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Description {
    Invalid,
    None,
    Transfer(ShellTransferOperation, PathBuf),
}

#[derive(Clone, Debug)]
enum Decision {
    None(Description),
    Grid,
    Tree {
        destination: PathBuf,
        operation: ShellTransferOperation,
        default_move_needs_volume_check: bool,
    },
}

impl Decision {
    fn accepted(&self) -> bool {
        !matches!(self, Self::None(_))
    }

    fn description(&self) -> Description {
        match self {
            Self::None(description) => description.clone(),
            Self::Grid => Description::Invalid,
            Self::Tree {
                destination,
                operation,
                ..
            } => Description::Transfer(*operation, destination.clone()),
        }
    }
}

fn lexical_components(path: &Path) -> Option<Vec<String>> {
    Some(
        crate::path_key::physical_path_spelling(path)?
            .split('/')
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect(),
    )
}

#[cfg(test)]
mod tests;

fn lexical_root(path: &Path) -> Option<String> {
    let text = crate::path_key::physical_path_spelling(path)?;
    let components = lexical_components(path)?;
    if text.starts_with("//") {
        Some(format!(r"\\{}\{}", components.first()?, components.get(1)?))
    } else if components.first()?.ends_with(':') {
        Some(components[0].clone())
    } else {
        None
    }
}

fn decide(
    hit: Option<Hit>,
    admission: bool,
    internal: Option<&InternalDrag>,
    external_has_paths: bool,
    source_allows_copy: bool,
    modifiers: u32,
) -> Decision {
    let Some(hit) = hit else {
        return Decision::None(Description::None);
    };
    let destination = match hit {
        Hit::TreeRow(path) => path,
        Hit::TreeNonTarget => return Decision::None(Description::None),
        Hit::ExistingDropArea => {
            return if internal.is_none() && external_has_paths && source_allows_copy {
                Decision::Grid
            } else {
                Decision::None(Description::None)
            };
        }
    };
    if !admission || !source_allows_copy {
        return Decision::None(Description::None);
    }
    let Some(internal) = internal else {
        return if external_has_paths {
            Decision::Tree {
                destination,
                operation: ShellTransferOperation::Copy,
                default_move_needs_volume_check: false,
            }
        } else {
            Decision::None(Description::None)
        };
    };
    if internal.paths.is_empty()
        || internal.excluded_virtual
        || internal.failed_paths != 0
        || modifiers & ALT != 0
        || modifiers & (CONTROL | SHIFT) == (CONTROL | SHIFT)
    {
        return Decision::None(Description::None);
    }
    let default = modifiers & (CONTROL | SHIFT) == 0;
    let operation = if modifiers & SHIFT != 0 {
        ShellTransferOperation::Move
    } else if modifiers & CONTROL != 0 {
        ShellTransferOperation::Copy
    } else {
        let destination_root = lexical_root(&destination);
        if destination_root.as_ref().is_some_and(|root| {
            internal
                .paths
                .iter()
                .all(|source| lexical_root(source).as_ref() == Some(root))
        }) {
            ShellTransferOperation::Move
        } else {
            ShellTransferOperation::Copy
        }
    };
    let Some(destination_parts) = lexical_components(&destination) else {
        return Decision::None(Description::None);
    };
    if internal.paths.iter().any(|source| {
        let Some(source_parts) = lexical_components(source) else {
            return true;
        };
        destination_parts.starts_with(&source_parts)
            || ((default || operation == ShellTransferOperation::Move)
                && source_parts
                    .len()
                    .checked_sub(1)
                    .is_some_and(|len| source_parts[..len] == destination_parts))
    }) {
        return Decision::None(Description::None);
    }
    Decision::Tree {
        destination,
        operation,
        default_move_needs_volume_check: default && operation == ShellTransferOperation::Move,
    }
}

trait DragData {
    fn has_paths(&self) -> bool;
    fn paths(&self) -> Result<Vec<PathBuf>, String>;
    fn set_description(&self, description: &Description) -> Result<(), String>;
}

struct DragSession {
    data: Rc<dyn DragData>,
    has_paths: bool,
    allows_copy: bool,
    description: RefCell<Option<Description>>,
}

impl DragSession {
    fn new(data: Rc<dyn DragData>, allows_copy: bool, internal: bool) -> Self {
        let has_paths = !internal && data.has_paths();
        Self {
            data,
            has_paths,
            allows_copy,
            description: RefCell::new(None),
        }
    }

    fn describe(&self, description: Description) {
        let changed = {
            let mut last = self.description.borrow_mut();
            if last.as_ref() == Some(&description) {
                false
            } else {
                *last = Some(description.clone());
                true
            }
        };
        if changed && let Err(error) = self.data.set_description(&description) {
            crate::logger::log(format!(
                "main_drop_target: drop description failed: {error}"
            ));
        }
    }

    fn drop_request(
        &self,
        decision: &Decision,
        internal: Option<&InternalDrag>,
    ) -> Result<Option<DropRequest>, String> {
        match decision {
            Decision::None(_) => Ok(None),
            Decision::Grid => {
                let paths = self.data.paths()?;
                if paths.is_empty() {
                    return Err("ドロップする項目がありません".into());
                }
                Ok(Some(DropRequest::GridFiles(paths)))
            }
            Decision::Tree {
                destination,
                operation,
                default_move_needs_volume_check,
            } => {
                let sources = match internal {
                    Some(internal) => internal.paths.clone(),
                    None => self.data.paths()?,
                };
                if sources.is_empty() {
                    return Err("ドロップする項目がありません".into());
                }
                Ok(Some(DropRequest::TreeTransfer(ShellTransferRequest {
                    operation: *operation,
                    default_move_needs_volume_check: *default_move_needs_volume_check,
                    sources,
                    destination: destination.clone(),
                })))
            }
        }
    }
}

#[cfg(windows)]
mod native;
#[cfg(windows)]
pub(crate) use native::{capture_window_metrics, register_root_drop_target};

#[cfg(not(windows))]
pub(crate) fn capture_window_metrics(_hwnd: isize) -> Option<WindowMetrics> {
    None
}

#[cfg(not(windows))]
pub(crate) fn register_root_drop_target(
    _hwnd: isize,
    _ctx: egui::Context,
    _shared: SharedMainDrop,
) -> Result<(), String> {
    Err("ドロップは Windows でのみ利用できます".into())
}
