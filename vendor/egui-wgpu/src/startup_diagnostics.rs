//! Optional observations of GPU setup and accepted root presentation.
//! Callbacks must publish without blocking; they are never rendering owners.

use std::sync::Arc;

/// Native operations whose return boundary can explain startup stalls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupGpuStage {
    WindowCreate,
    Instance,
    AdapterEnumeration,
    AdapterSelection,
    Device,
    SurfaceCreate,
    SurfaceCapabilities,
    PipelineCreate,
    SurfaceConfigure,
    TextureDelivery,
    Buffers,
    Encode,
    SurfaceAcquire,
    QueueSubmit,
    Present,
}

/// A begin is deliberately left unmatched if its native operation never returns.
#[derive(Clone, Debug)]
pub enum StartupDiagnosticEvent {
    Begin(StartupGpuStage),
    End {
        stage: StartupGpuStage,
        success: bool,
    },
    AdapterSelected(wgpu::AdapterInfo),
    /// Emitted only after the real root `SurfaceTexture::present` returns.
    RootPresented {
        frame: u64,
        normal: bool,
    },
}

pub type StartupDiagnosticsCallback = Arc<dyn Fn(StartupDiagnosticEvent) + Send + Sync>;

/// Identity of the root pass whose shapes are submitted by eframe.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RootStartupFrameTag {
    pub frame: u64,
    pub pass: u64,
    pub normal: bool,
}

fn tag_id() -> egui::Id {
    egui::Id::new("miv.root_startup_frame_tag")
}

/// Call once on every root App pass with that pass's actual shell state.
/// Child viewports cannot overwrite the root's tag.
pub fn set_root_startup_frame_tag(ctx: &egui::Context, normal: bool) {
    if ctx.viewport_id() != egui::ViewportId::ROOT {
        return;
    }
    let tag = RootStartupFrameTag {
        frame: ctx.cumulative_frame_nr().saturating_add(1),
        pass: ctx.cumulative_pass_nr().saturating_add(1),
        normal,
    };
    ctx.data_mut(|data| data.insert_temp(tag_id(), tag));
}

/// Capture after Context::run has accepted its final pass. Missing final tags
/// are fail-closed: an earlier discarded Normal pass cannot prove readiness.
pub fn take_root_startup_frame_tag(ctx: &egui::Context) -> RootStartupFrameTag {
    let frame = ctx.cumulative_frame_nr_for(egui::ViewportId::ROOT);
    let pass = ctx.cumulative_pass_nr_for(egui::ViewportId::ROOT);
    let published = ctx.data_mut(|data| data.remove_temp::<RootStartupFrameTag>(tag_id()));
    RootStartupFrameTag {
        frame,
        pass,
        normal: published.is_some_and(|tag| tag.frame == frame && tag.pass == pass && tag.normal),
    }
}

#[derive(Default)]
pub(crate) struct RootPresentObserver {
    first_returned: bool,
    normal_returned: bool,
}

impl RootPresentObserver {
    pub(crate) fn pending(&self) -> bool {
        !self.normal_returned
    }
    pub(crate) fn observes(
        &self,
        viewport: egui::ViewportId,
        tag: Option<RootStartupFrameTag>,
    ) -> bool {
        viewport == egui::ViewportId::ROOT
            && (!self.first_returned
                || (!self.normal_returned && tag.is_some_and(|tag| tag.normal)))
    }

    /// Called at the present-return boundary, never at acquire or submit.
    pub(crate) fn returned(
        &mut self,
        viewport: egui::ViewportId,
        tag: Option<RootStartupFrameTag>,
    ) -> Option<StartupDiagnosticEvent> {
        if !self.observes(viewport, tag) {
            return None;
        }
        self.first_returned = true;
        let normal = tag.is_some_and(|tag| tag.normal);
        self.normal_returned |= normal;
        Some(StartupDiagnosticEvent::RootPresented {
            frame: tag.map_or(0, |tag| tag.frame),
            normal,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_pass_tag_wins_and_missing_final_tag_is_not_normal() {
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |ctx| {
            set_root_startup_frame_tag(ctx, ctx.current_pass_index() == 0);
            if ctx.current_pass_index() == 0 {
                ctx.request_discard("exercise final overlay pass");
            }
        });
        assert!(!take_root_startup_frame_tag(&ctx).normal);
        let _ = ctx.run(Default::default(), |ctx| {
            set_root_startup_frame_tag(ctx, ctx.current_pass_index() > 0);
            if ctx.current_pass_index() == 0 {
                ctx.request_discard("exercise final normal pass");
            }
        });
        assert!(take_root_startup_frame_tag(&ctx).normal);
        let _ = ctx.run(Default::default(), |ctx| {
            if ctx.current_pass_index() == 0 {
                set_root_startup_frame_tag(ctx, true);
                ctx.request_discard("final pass intentionally has no tag");
            }
        });
        assert!(!take_root_startup_frame_tag(&ctx).normal);
        let _ = ctx.run(Default::default(), |ctx| {
            set_root_startup_frame_tag(ctx, true)
        });
        assert!(take_root_startup_frame_tag(&ctx).normal);
        assert!(!take_root_startup_frame_tag(&ctx).normal);
    }

    #[test]
    fn root_return_and_normal_return_are_independent_one_shots() {
        let mut observer = RootPresentObserver::default();
        let root = egui::ViewportId::ROOT;
        let child = egui::ViewportId::from_hash_of("child");
        let tag = RootStartupFrameTag {
            frame: 2,
            pass: 3,
            normal: true,
        };
        assert!(observer.returned(child, Some(tag)).is_none());
        assert!(matches!(
            observer.returned(root, None),
            Some(StartupDiagnosticEvent::RootPresented { normal: false, .. })
        ));
        assert!(observer.returned(root, None).is_none());
        assert!(matches!(
            observer.returned(root, Some(tag)),
            Some(StartupDiagnosticEvent::RootPresented { normal: true, .. })
        ));
        assert!(observer.returned(root, Some(tag)).is_none());
    }
}
