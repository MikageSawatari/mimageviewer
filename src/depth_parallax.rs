//! Context-owned 3D photo state. Pixel selection remains with the fullscreen resolver.
use crate::app::ViewerContextId;
pub(crate) use crate::depth_estimation::{DepthGuide, DepthPending, DepthSourceKey};
use crate::depth_parallax_wgpu::{
    DepthColor, DepthGpu, DepthReadback, DepthRenderResources, DepthTexture,
};
use std::collections::VecDeque;
use std::sync::{Arc, Weak};
use std::time::Instant;

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum MotionKind {
    Mouse,
    #[default]
    Horizontal,
    Circle,
    FigureEight,
    Vertical,
    #[serde(other)]
    Unknown,
}
impl MotionKind {
    pub fn normalized(self) -> Self {
        if self == Self::Unknown {
            Self::Horizontal
        } else {
            self
        }
    }
    pub fn all() -> &'static [Self] {
        &[
            Self::Mouse,
            Self::Horizontal,
            Self::Circle,
            Self::FigureEight,
            Self::Vertical,
        ]
    }
    pub fn label(self) -> &'static str {
        match self.normalized() {
            Self::Mouse => "マウス追従",
            Self::Horizontal => "左右",
            Self::Circle => "円",
            Self::FigureEight => "8 の字",
            Self::Vertical => "上下",
            Self::Unknown => unreachable!(),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct DepthParallaxSettings {
    pub motion: MotionKind,
    pub strength: f32,
    pub period_secs: f32,
    pub focus: f32,
    pub shape_depth: bool,
}
impl Default for DepthParallaxSettings {
    fn default() -> Self {
        Self {
            motion: MotionKind::Horizontal,
            strength: 0.02,
            period_secs: 3.0,
            focus: 0.5,
            shape_depth: true,
        }
    }
}
impl DepthParallaxSettings {
    pub fn normalize(&mut self) {
        self.motion = self.motion.normalized();
        self.strength = finite_clamp(self.strength, 0.005, 0.05, 0.02);
        self.period_secs = finite_clamp(self.period_secs, 1.0, 10.0, 3.0);
        self.focus = finite_clamp(self.focus, 0.0, 1.0, 0.5);
    }
}
fn finite_clamp(v: f32, low: f32, high: f32, default: f32) -> f32 {
    if v.is_finite() {
        v.clamp(low, high)
    } else {
        default
    }
}

/// Pure periodic path, shared with export. Phase 0 and 1 have exactly the same value.
pub fn motion_offset(kind: MotionKind, strength: f32, phase: f32) -> egui::Vec2 {
    let theta = if phase.is_finite() {
        phase.rem_euclid(1.0) * std::f32::consts::TAU
    } else {
        0.0
    };
    let s = finite_clamp(strength, 0.0, 0.05, 0.02);
    let (sin, cos) = theta.sin_cos();
    match kind.normalized() {
        MotionKind::Mouse => egui::Vec2::ZERO,
        MotionKind::Horizontal => egui::vec2(s * sin, 0.0),
        MotionKind::Circle => egui::vec2(s * cos, s * 0.6 * sin),
        MotionKind::FigureEight => egui::vec2(s * sin, s * 0.5 * (2.0 * theta).sin()),
        MotionKind::Vertical => egui::vec2(0.0, s * 0.7 * sin),
        MotionKind::Unknown => unreachable!(),
    }
}

#[derive(Default, Debug)]
pub struct MouseFollow {
    pub current: egui::Vec2,
    pub target: egui::Vec2,
}
impl MouseFollow {
    /// Negate pointer displacement for the reference shader's inverse lookup
    /// (`source = pixel - offset * (depth - focus)`). Moving the pointer right/down
    /// shifts foreground left/up, as when peeking around it from the pointer side.
    pub fn update(
        &mut self,
        pointer: Option<egui::Pos2>,
        image: egui::Rect,
        over_panel: bool,
        strength: f32,
        dt: f32,
    ) -> bool {
        if !over_panel {
            self.target = pointer
                .filter(|_| image.width() > 0.0 && image.height() > 0.0)
                .map_or(egui::Vec2::ZERO, |p| {
                    let n = (p - image.center()) / (image.size() * 0.5);
                    -egui::vec2(n.x.clamp(-1.0, 1.0), 0.6 * n.y.clamp(-1.0, 1.0)) * strength
                });
        }
        let dt = finite_clamp(dt, 0.0, 1.0, 0.0);
        self.current += (self.target - self.current) * (1.0 - (-dt / 0.15).exp());
        let moving = (self.target - self.current).length_sq() > 1e-10;
        if !moving {
            self.current = self.target;
        }
        moving
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct DepthEntryInput {
    pub image_viewer: bool,
    pub is_video: bool,
    pub is_animation: bool,
    pub spread_double: bool,
    pub continuous_reading: bool,
    pub main_viewer_presentation: bool,
    pub wgpu_available: bool,
    pub editing: bool,
    pub page_turning: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DepthEntryBlock {
    NotImageViewer,
    Video,
    Animation,
    Spread,
    ContinuousReading,
    Detached,
    NoWgpu,
    Editing,
    PageTurning,
}
impl DepthEntryBlock {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::NotImageViewer => "3D 写真は画像の表示中に使えます",
            Self::Video => "動画は 3D 写真に対応していません",
            Self::Animation => "アニメーション画像は 3D 写真に対応していません",
            Self::Spread => "3D 写真は 1 ページ表示で使えます",
            Self::ContinuousReading => "3D 写真はページ表示で使えます",
            Self::Detached => "3D 写真はメインウィンドウの画像表示で使えます",
            Self::NoWgpu => "3D 写真には GPU 描画が必要です",
            Self::Editing => "編集モードを終了してから 3D 写真を使ってください",
            Self::PageTurning => "ページめくりの完了後に 3D 写真を使ってください",
        }
    }
}
pub(crate) fn entry_allowed(i: DepthEntryInput) -> Result<(), DepthEntryBlock> {
    let block = if i.is_video {
        Some(DepthEntryBlock::Video)
    } else if !i.image_viewer {
        Some(DepthEntryBlock::NotImageViewer)
    } else if i.is_animation {
        Some(DepthEntryBlock::Animation)
    } else if i.spread_double {
        Some(DepthEntryBlock::Spread)
    } else if i.continuous_reading {
        Some(DepthEntryBlock::ContinuousReading)
    } else if !i.main_viewer_presentation {
        Some(DepthEntryBlock::Detached)
    } else if !i.wgpu_available {
        Some(DepthEntryBlock::NoWgpu)
    } else if i.editing {
        Some(DepthEntryBlock::Editing)
    } else if i.page_turning {
        Some(DepthEntryBlock::PageTurning)
    } else {
        None
    };
    block.map_or(Ok(()), Err)
}

pub(crate) enum DepthSourceMaterial {
    Ready { texture: egui::TextureHandle },
    NotYet(DepthSourceWait),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DepthSourceWait {
    ThumbnailFallback,
    PageTransition,
    IncompleteComposite,
    OriginalHold,
    AnnotationBake,
    NativeOnly,
    TexturePending,
}
pub(crate) enum DepthSourcePhase {
    WaitingForStableTexture,
    ReadingBack {
        key: DepthSourceKey,
        readback: DepthReadback,
    },
    Estimating {
        key: DepthSourceKey,
        pending: DepthPending,
    },
    Ready {
        key: DepthSourceKey,
    },
    Failed {
        key: DepthSourceKey,
        reason: String,
    },
}
impl DepthSourcePhase {
    pub(crate) fn key(&self) -> Option<DepthSourceKey> {
        match self {
            Self::WaitingForStableTexture => None,
            Self::ReadingBack { key, .. }
            | Self::Estimating { key, .. }
            | Self::Ready { key }
            | Self::Failed { key, .. } => Some(*key),
        }
    }
}

/// Only depth maps are retained across page changes; the large color mip chain
/// has exactly one owner slot. LRU order is oldest to newest.
#[derive(Default)]
pub(crate) struct DepthLru(VecDeque<(DepthSourceKey, Arc<DepthTexture>)>);
impl DepthLru {
    pub(crate) fn get(&mut self, key: DepthSourceKey) -> Option<Arc<DepthTexture>> {
        let index = self.0.iter().position(|(k, _)| *k == key)?;
        let entry = self.0.remove(index)?;
        let value = Arc::clone(&entry.1);
        self.0.push_back(entry);
        Some(value)
    }
    pub(crate) fn insert(&mut self, key: DepthSourceKey, value: Arc<DepthTexture>) {
        self.0.retain(|(k, _)| *k != key);
        self.0.push_back((key, value));
        while self.0.len() > 4 {
            self.0.pop_front();
        }
    }
    pub(crate) fn resident_bytes(&self) -> u64 {
        self.0.iter().map(|(_, v)| v.byte_size()).sum()
    }
}

pub(crate) struct DepthViewState {
    pub context: ViewerContextId,
    pub panel_visible: bool,
    pub started_at: Instant,
    // Continuous pending presentation only; readiness/cancellation still belongs to source.
    pub wait_started_at: Instant,
    pub mouse: MouseFollow,
    pub source: DepthSourcePhase,
    pub lru: DepthLru,
    pub wait_reason: DepthSourceWait,
    gpu: Option<Arc<DepthGpu>>,
    color: Option<Arc<DepthColor>>,
    rendered: Option<Arc<DepthRenderResources>>,
    renderer: Weak<egui::epaint::mutex::RwLock<egui_wgpu::Renderer>>,
    // A discard/retry pass or a new source in the same viewport frame must not
    // multiply the bounded R16 upload. This is accounting, not a source phase.
    last_depth_upload_frame: Option<u64>,
}
impl DepthViewState {
    pub(crate) fn new(context: ViewerContextId) -> Self {
        let now = Instant::now();
        Self {
            context,
            panel_visible: true,
            started_at: now,
            wait_started_at: now,
            mouse: MouseFollow::default(),
            source: DepthSourcePhase::WaitingForStableTexture,
            lru: DepthLru::default(),
            wait_reason: DepthSourceWait::TexturePending,
            gpu: None,
            color: None,
            rendered: None,
            renderer: Weak::new(),
            last_depth_upload_frame: None,
        }
    }
    pub(crate) fn attach_renderer(&mut self, state: &egui_wgpu::RenderState) {
        self.attach_renderer_handle(&state.renderer);
    }
    pub(crate) fn attach_renderer_handle(
        &mut self,
        renderer: &Arc<egui::epaint::mutex::RwLock<egui_wgpu::Renderer>>,
    ) {
        self.renderer = Arc::downgrade(renderer);
    }
    pub(crate) fn clear_slot(&self) {
        if let Some(renderer) = self.renderer.upgrade() {
            crate::depth_parallax_wgpu::clear_depth_slot(
                &mut renderer.write().callback_resources,
                self.context,
            );
        }
    }
    pub(crate) fn wait_for_source(&mut self, reason: DepthSourceWait) {
        // Temporary display holds are not permission to retry a failed key.
        if !matches!(self.source, DepthSourcePhase::Failed { .. }) {
            if matches!(self.source, DepthSourcePhase::Ready { .. }) {
                self.wait_started_at = Instant::now();
            }
            self.source = DepthSourcePhase::WaitingForStableTexture;
        }
        self.color = None;
        self.rendered = None;
        self.wait_reason = reason;
        self.clear_slot();
    }
    pub(crate) fn select_source(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        key: DepthSourceKey,
    ) -> Result<(), String> {
        if self.source.key() == Some(key) {
            return Ok(());
        }
        if matches!(
            self.source,
            DepthSourcePhase::Ready { .. } | DepthSourcePhase::Failed { .. }
        ) {
            self.wait_started_at = Instant::now();
        }
        self.clear_slot();
        self.source = DepthSourcePhase::WaitingForStableTexture;
        self.rendered = None;
        self.color = None;
        let gpu = self
            .gpu
            .get_or_insert_with(|| Arc::new(DepthGpu::new(device)))
            .clone();
        let color = match gpu.copy_color(device, queue, texture) {
            Ok(color) => Arc::new(color),
            Err(reason) => {
                self.source = DepthSourcePhase::Failed {
                    key,
                    reason: reason.clone(),
                };
                return Err(reason);
            }
        };
        if let Some(depth) = self.lru.get(key) {
            self.rendered = Some(Arc::new(DepthRenderResources::new(
                device,
                self.context,
                key,
                color.clone(),
                depth,
            )));
            self.source = DepthSourcePhase::Ready { key };
        } else {
            self.source = DepthSourcePhase::ReadingBack {
                key,
                readback: gpu.readback(device, queue, &color),
            };
        }
        self.color = Some(color);
        Ok(())
    }
    /// No waiting or I/O. A single small R16 upload is allowed when a map arrives.
    pub(crate) fn advance(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        egui: &egui::Context,
        viewport: egui::ViewportId,
        submit: impl FnOnce(DepthSourceKey, Vec<u8>, DepthGuide) -> Result<DepthPending, String>,
    ) {
        let completion = match &mut self.source {
            DepthSourcePhase::ReadingBack { readback, .. } => {
                let _ = device.poll(wgpu::PollType::Poll);
                if egui.input(|input| input.viewport().minimized != Some(true)) {
                    egui.request_repaint_of(viewport);
                }
                readback.try_finish()
            }
            _ => None,
        };
        if let Some(result) = completion {
            let key = self.source.key().unwrap();
            self.source = match result.and_then(|(input, guide)| submit(key, input, guide)) {
                Ok(pending) => DepthSourcePhase::Estimating { key, pending },
                Err(reason) => {
                    crate::logger::log(format!("[Depth] {reason}"));
                    DepthSourcePhase::Failed { key, reason }
                }
            };
        }
        let frame = egui.cumulative_frame_nr_for(viewport);
        if matches!(self.source, DepthSourcePhase::Estimating { .. })
            && self.last_depth_upload_frame == Some(frame)
        {
            // Keep the completed reply in its existing per-request receiver.
            // Consuming it now would require a second CPU-ready owner/phase.
            if egui.input(|input| input.viewport().minimized != Some(true)) {
                egui.request_repaint_of(viewport);
            }
            return;
        }
        let result = match &self.source {
            DepthSourcePhase::Estimating { key, pending } => pending.try_result(self.context, *key),
            _ => None,
        };
        if let Some(result) = result {
            let key = self.source.key().unwrap();
            match result.and_then(|map| DepthTexture::upload(device, queue, map)) {
                Ok(depth) => {
                    self.last_depth_upload_frame = Some(frame);
                    let depth = Arc::new(depth);
                    self.lru.insert(key, depth.clone());
                    if let Some(color) = &self.color {
                        self.rendered = Some(Arc::new(DepthRenderResources::new(
                            device,
                            self.context,
                            key,
                            color.clone(),
                            depth,
                        )));
                        self.source = DepthSourcePhase::Ready { key };
                    }
                }
                Err(reason) => {
                    crate::logger::log(format!("[Depth] {reason}"));
                    self.source = DepthSourcePhase::Failed { key, reason };
                }
            }
        }
    }
    pub(crate) fn ready(&self) -> Option<Arc<DepthRenderResources>> {
        self.rendered.clone()
    }
    pub(crate) fn publish(&self, resources: &mut egui_wgpu::CallbackResources) {
        if let Some(ready) = self.ready() {
            resources.insert(crate::depth_parallax_wgpu::DepthRenderResourcesRef(ready));
        }
    }
    pub(crate) fn resident_bytes(&self) -> u64 {
        self.color_bytes() + self.depth_bytes() + self.readback_bytes()
    }
    pub(crate) fn color_bytes(&self) -> u64 {
        self.color.as_ref().map_or(0, |v| v.byte_size())
    }
    pub(crate) fn depth_bytes(&self) -> u64 {
        self.lru.resident_bytes()
    }
    pub(crate) fn readback_bytes(&self) -> u64 {
        match &self.source {
            DepthSourcePhase::ReadingBack { readback, .. } => readback.byte_size(),
            _ => 0,
        }
    }
}
impl Drop for DepthViewState {
    fn drop(&mut self) {
        self.clear_slot();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn depth_failed_key_survives_temporary_display_waits() {
        let key = DepthSourceKey {
            texture_id: egui::TextureId::Managed(99),
        };
        let mut state = DepthViewState::new(ViewerContextId::for_test(42));
        state.source = DepthSourcePhase::Failed {
            key,
            reason: "test failure".to_owned(),
        };
        for wait in [
            DepthSourceWait::OriginalHold,
            DepthSourceWait::TexturePending,
            DepthSourceWait::PageTransition,
        ] {
            state.wait_for_source(wait);
            assert!(
                matches!(&state.source, DepthSourcePhase::Failed { key: failed_key, reason } if *failed_key == key && reason == "test failure")
            );
        }
    }
    #[test]
    fn periodic_motion_and_bounds() {
        for &kind in MotionKind::all() {
            assert_eq!(
                motion_offset(kind, 0.02, 0.0),
                motion_offset(kind, 0.02, 1.0)
            );
            for i in 0..1000 {
                let p = motion_offset(kind, 0.02, i as f32 / 1000.);
                assert!(p.x.abs() <= 0.020001 && p.y.abs() <= 0.020001);
            }
        }
    }
    #[test]
    fn entry_rejects_each_incompatible_state() {
        let good = DepthEntryInput {
            image_viewer: true,
            main_viewer_presentation: true,
            wgpu_available: true,
            ..Default::default()
        };
        assert_eq!(entry_allowed(good), Ok(()));
        for (input, block) in [
            (
                DepthEntryInput {
                    is_video: true,
                    ..good
                },
                DepthEntryBlock::Video,
            ),
            (
                DepthEntryInput {
                    is_animation: true,
                    ..good
                },
                DepthEntryBlock::Animation,
            ),
            (
                DepthEntryInput {
                    spread_double: true,
                    ..good
                },
                DepthEntryBlock::Spread,
            ),
            (
                DepthEntryInput {
                    continuous_reading: true,
                    ..good
                },
                DepthEntryBlock::ContinuousReading,
            ),
            (
                DepthEntryInput {
                    image_viewer: false,
                    ..good
                },
                DepthEntryBlock::NotImageViewer,
            ),
            (
                DepthEntryInput {
                    main_viewer_presentation: false,
                    ..good
                },
                DepthEntryBlock::Detached,
            ),
            (
                DepthEntryInput {
                    editing: true,
                    ..good
                },
                DepthEntryBlock::Editing,
            ),
            (
                DepthEntryInput {
                    page_turning: true,
                    ..good
                },
                DepthEntryBlock::PageTurning,
            ),
            (
                DepthEntryInput {
                    wgpu_available: false,
                    ..good
                },
                DepthEntryBlock::NoWgpu,
            ),
        ] {
            assert_eq!(entry_allowed(input), Err(block));
        }
    }
    #[test]
    fn mouse_panel_freeze_and_return_to_center() {
        let mut mouse = MouseFollow::default();
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100., 100.));
        mouse.update(Some(egui::pos2(100., 100.)), rect, false, 0.02, 0.15);
        assert_eq!(mouse.target, egui::vec2(-0.02, -0.012));
        assert!(mouse.current.x < 0. && mouse.current.y < 0.);
        let target = mouse.target;
        mouse.update(Some(rect.min), rect, true, 0.02, 0.15);
        assert_eq!(mouse.target, target);
        mouse.update(None, rect, false, 0.02, 0.15);
        assert_eq!(mouse.target, egui::Vec2::ZERO);
    }
    #[test]
    fn settings_normalize_nonfinite_and_unknown() {
        let mut settings = DepthParallaxSettings {
            motion: MotionKind::Unknown,
            strength: f32::NAN,
            period_secs: f32::INFINITY,
            focus: -1.,
            shape_depth: true,
        };
        settings.normalize();
        assert_eq!(settings.strength, 0.02);
        assert_eq!(settings.period_secs, 3.);
        assert_eq!(settings.focus, 0.);
        assert_eq!(settings.motion, MotionKind::Horizontal);
    }
}
