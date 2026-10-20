//! The single physical desktop-coordinate boundary for detached windows.
//!
//! Calls that move a window synchronously reenter winit's window procedure.
//! Never hold a winit/egui/painter lock or borrow across `apply_outer_rect`.

use crate::settings::DetachedViewerWindowPlacement;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PhysicalRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl PhysicalRect {
    pub fn width(self) -> i32 {
        self.right.saturating_sub(self.left)
    }
    pub fn height(self) -> i32 {
        self.bottom.saturating_sub(self.top)
    }
    pub fn is_valid(self) -> bool {
        self.right > self.left
            && self.bottom > self.top
            && i64::from(self.right) - i64::from(self.left) <= i64::from(i32::MAX)
            && i64::from(self.bottom) - i64::from(self.top) <= i64::from(i32::MAX)
    }
    fn translated(self, x: i32, y: i32) -> Self {
        Self {
            left: self.left.saturating_add(x),
            top: self.top.saturating_add(y),
            right: self.right.saturating_add(x),
            bottom: self.bottom.saturating_add(y),
        }
    }
    fn intersection_area(self, other: Self) -> i64 {
        let w =
            (i64::from(self.right.min(other.right)) - i64::from(self.left.max(other.left))).max(0);
        let h =
            (i64::from(self.bottom.min(other.bottom)) - i64::from(self.top.max(other.top))).max(0);
        w * h
    }
    fn contains(self, x: f64, y: f64) -> bool {
        x >= f64::from(self.left)
            && x < f64::from(self.right)
            && y >= f64::from(self.top)
            && y < f64::from(self.bottom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PhysicalWindowPlacement {
    pub outer: PhysicalRect,
    #[serde(default)]
    pub maximized: bool,
}

impl PhysicalWindowPlacement {
    pub fn is_sane(self) -> bool {
        self.outer.is_valid() && self.outer.width() <= 65_536 && self.outer.height() <= 65_536
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonitorFacts {
    pub id: isize,
    pub rc_monitor: PhysicalRect,
    pub rc_work: PhysicalRect,
    pub dpi: u32,
}

impl MonitorFacts {
    pub fn scale(self) -> f32 {
        self.dpi as f32 / 96.0
    }
}

/// Total outer-minus-client extent, rather than style-derived margins.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameInsets {
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BuilderPlacement {
    pub position: [f32; 2],
    pub inner_size: [f32; 2],
}

pub fn builder_placement(
    placement: PhysicalWindowPlacement,
    monitor: MonitorFacts,
    frame: FrameInsets,
    ui_scale: f32,
) -> BuilderPlacement {
    let scale = monitor.scale() * ui_scale;
    BuilderPlacement {
        position: [
            placement.outer.left as f32 / scale,
            placement.outer.top as f32 / scale,
        ],
        inner_size: [
            (placement.outer.width() - frame.width).max(1) as f32 / scale,
            (placement.outer.height() - frame.height).max(1) as f32 / scale,
        ],
    }
}

pub fn default_placement(
    main_outer: PhysicalRect,
    monitor: MonitorFacts,
    frame: FrameInsets,
) -> PhysicalWindowPlacement {
    let s = monitor.scale();
    let x = main_outer.left.saturating_add((48.0 * s).round() as i32);
    let y = main_outer.top.saturating_add((48.0 * s).round() as i32);
    PhysicalWindowPlacement {
        outer: PhysicalRect {
            left: x,
            top: y,
            right: x
                .saturating_add((960.0 * s).round() as i32)
                .saturating_add(frame.width),
            bottom: y
                .saturating_add((720.0 * s).round() as i32)
                .saturating_add(frame.height),
        },
        maximized: false,
    }
}

pub fn offset_placement(
    base: PhysicalWindowPlacement,
    steps: usize,
    monitor: MonitorFacts,
) -> PhysicalWindowPlacement {
    let delta = (40.0 * monitor.scale() as f64 * steps as f64).round() as i32;
    PhysicalWindowPlacement {
        outer: base.outer.translated(delta, delta),
        maximized: false,
    }
}

/// `frames` contains the ordinary decorated extents at each matching monitor DPI.
/// No candidate means the caller chooses its existing default seed.
pub fn migrate_legacy_placement(
    legacy: DetachedViewerWindowPlacement,
    monitors: &[MonitorFacts],
    frames: &[FrameInsets],
) -> Option<PhysicalWindowPlacement> {
    if !legacy.is_sane() || monitors.len() != frames.len() {
        return None;
    }
    let mut best: Option<(i64, PhysicalWindowPlacement)> = None;
    for (monitor, frame) in monitors.iter().zip(frames) {
        if monitor.dpi == 0 {
            continue;
        }
        let s = f64::from(monitor.dpi) / 96.0;
        let cx = (f64::from(legacy.x) + f64::from(legacy.w) / 2.0) * s;
        let cy = (f64::from(legacy.y) + f64::from(legacy.h) / 2.0) * s;
        if !monitor.rc_monitor.contains(cx, cy) {
            continue;
        }
        let left = (f64::from(legacy.x) * s).round() as i32;
        let top = (f64::from(legacy.y) * s).round() as i32;
        let old_rect = PhysicalRect {
            left,
            top,
            right: left.saturating_add((f64::from(legacy.w) * s).round() as i32),
            bottom: top.saturating_add((f64::from(legacy.h) * s).round() as i32),
        };
        let placement = PhysicalWindowPlacement {
            outer: PhysicalRect {
                right: old_rect.right.saturating_add(frame.width),
                bottom: old_rect.bottom.saturating_add(frame.height),
                ..old_rect
            },
            maximized: legacy.maximized,
        };
        let area = old_rect.intersection_area(monitor.rc_monitor);
        if placement.is_sane() && best.as_ref().is_none_or(|(previous, _)| area > *previous) {
            best = Some((area, placement));
        }
    }
    best.map(|(_, placement)| placement)
}

pub fn title_bar_on_some_monitor(rect: PhysicalRect) -> bool {
    if !rect.is_valid() {
        return false;
    }
    let band = PhysicalRect {
        bottom: rect.top.saturating_add(32).min(rect.bottom),
        ..rect
    };
    native::monitor_for_rect(band, false).is_some()
}

fn normal_screen_rect(normal: PhysicalRect, monitors: &[MonitorFacts]) -> Option<PhysicalRect> {
    if !normal.is_valid() {
        return None;
    }
    monitors
        .iter()
        .filter_map(|m| {
            let screen = normal.translated(
                m.rc_work.left - m.rc_monitor.left,
                m.rc_work.top - m.rc_monitor.top,
            );
            let own = screen.intersection_area(m.rc_monitor);
            let other = monitors
                .iter()
                .filter(|other| other.id != m.id)
                .map(|other| screen.intersection_area(other.rc_monitor))
                .max()
                .unwrap_or(0);
            (own > 0 && own >= other).then_some((own, screen))
        })
        .max_by_key(|(area, _)| *area)
        .map(|(_, screen)| screen)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacementStage {
    Preconditions,
    ShrinkAtCurrentDpi,
    MoveToTargetDpi,
    Final,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacementMismatch {
    pub stage: PlacementStage,
    pub target: PhysicalRect,
    pub actual: Option<PhysicalRect>,
    pub expected_dpi: Option<u32>,
    pub actual_dpi: Option<u32>,
    pub expected_monitor: Option<isize>,
    pub actual_monitor: Option<isize>,
    pub reason: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlacementApplied {
    pub moved: bool,
}

/// Small synchronous Win32 surface, also used by the winit-behavior fake.
pub trait WindowGeometry {
    fn monitor_for_rect(&self, rect: PhysicalRect) -> Option<MonitorFacts>;
    fn outer_rect(&self, hwnd: isize) -> Option<PhysicalRect>;
    fn client_size(&self, hwnd: isize) -> Option<[i32; 2]>;
    fn dpi(&self, hwnd: isize) -> Option<u32>;
    fn monitor_for_window(&self, hwnd: isize) -> Option<MonitorFacts>;
    fn zoomed(&self, hwnd: isize) -> bool;
    /// `move_only` uses SWP_NOSIZE; resizing before a DPI change is a separate step.
    fn set_outer_rect(&mut self, hwnd: isize, rect: PhysicalRect, move_only: bool) -> bool;
}

fn mismatch(
    api: &impl WindowGeometry,
    hwnd: isize,
    target: PhysicalRect,
    monitor: Option<MonitorFacts>,
    stage: PlacementStage,
    reason: &'static str,
) -> PlacementMismatch {
    PlacementMismatch {
        stage,
        target,
        actual: api.outer_rect(hwnd),
        actual_dpi: api.dpi(hwnd),
        expected_dpi: monitor.map(|m| m.dpi),
        expected_monitor: monitor.map(|m| m.id),
        actual_monitor: api.monitor_for_window(hwnd).map(|m| m.id),
        reason,
    }
}

fn centered(work: PhysicalRect, width: i32, height: i32) -> PhysicalRect {
    let left = work.left + (work.width() - width) / 2;
    let top = work.top + (work.height() - height) / 2;
    PhysicalRect {
        left,
        top,
        right: left.saturating_add(width),
        bottom: top.saturating_add(height),
    }
}

/// Fits both the pre-change outer rect and winit's client*r rebuilt outer rect
/// within half the target work area. The source rect must fit its work area too.
fn staging_client(
    source: MonitorFacts,
    target: MonitorFacts,
    frame: FrameInsets,
    ratio: f64,
) -> Option<[i32; 2]> {
    let dimension = |preferred: i32, source_extent: i32, target_extent: i32, decoration: i32| {
        let extent = (f64::from(preferred))
            .min(f64::from(source_extent) / 2.0 - f64::from(decoration))
            .min(f64::from(target_extent) / 2.0 - f64::from(decoration))
            .min(f64::from(target_extent) / (2.0 * ratio) - f64::from(decoration));
        (extent.floor() as i32).max(0)
    };
    let w = dimension(
        320,
        source.rc_work.width(),
        target.rc_work.width(),
        frame.width,
    );
    let h = dimension(
        240,
        source.rc_work.height(),
        target.rc_work.height(),
        frame.height,
    );
    (w > 0 && h > 0).then_some([w, h])
}

pub fn apply_outer_rect_with(
    api: &mut impl WindowGeometry,
    hwnd: isize,
    target: PhysicalRect,
) -> Result<PlacementApplied, PlacementMismatch> {
    let monitor = api.monitor_for_rect(target);
    let fail = |api: &_, stage, reason| mismatch(api, hwnd, target, monitor, stage, reason);
    let Some(m) = monitor.filter(|m| m.dpi > 0 && m.rc_work.is_valid()) else {
        return Err(fail(
            api,
            PlacementStage::Preconditions,
            "target monitor unavailable",
        ));
    };
    if !target.is_valid() || api.zoomed(hwnd) {
        return Err(fail(
            api,
            PlacementStage::Preconditions,
            "invalid target or maximized window",
        ));
    }
    let (Some(initial), Some(dpi)) = (api.outer_rect(hwnd), api.dpi(hwnd).filter(|dpi| *dpi > 0))
    else {
        return Err(fail(
            api,
            PlacementStage::Preconditions,
            "window rect or DPI unavailable",
        ));
    };
    if initial == target && dpi == m.dpi {
        return Ok(PlacementApplied { moved: false });
    }
    if dpi != m.dpi {
        let (Some(k), Some(client)) = (api.monitor_for_rect(initial), api.client_size(hwnd)) else {
            return Err(fail(
                api,
                PlacementStage::ShrinkAtCurrentDpi,
                "source monitor or client unavailable",
            ));
        };
        let frame = FrameInsets {
            width: initial.width() - client[0],
            height: initial.height() - client[1],
        };
        let Some(small) = staging_client(k, m, frame, f64::from(m.dpi) / f64::from(dpi)) else {
            return Err(fail(
                api,
                PlacementStage::ShrinkAtCurrentDpi,
                "no staging size fits work area",
            ));
        };
        let at_source = centered(k.rc_work, small[0] + frame.width, small[1] + frame.height);
        if !api.set_outer_rect(hwnd, at_source, false)
            || api.dpi(hwnd) != Some(dpi)
            || api.client_size(hwnd) != Some(small)
        {
            return Err(fail(
                api,
                PlacementStage::ShrinkAtCurrentDpi,
                "source DPI or small client did not settle synchronously",
            ));
        }
        let at_target = centered(m.rc_work, at_source.width(), at_source.height());
        if !api.set_outer_rect(hwnd, at_target, true)
            || api.dpi(hwnd) != Some(m.dpi)
            || api.monitor_for_window(hwnd).map(|k| k.id) != Some(m.id)
        {
            return Err(fail(
                api,
                PlacementStage::MoveToTargetDpi,
                "target DPI or monitor did not settle synchronously",
            ));
        }
    }
    if !api.set_outer_rect(hwnd, target, false)
        || api.outer_rect(hwnd) != Some(target)
        || api.dpi(hwnd) != Some(m.dpi)
    {
        return Err(fail(
            api,
            PlacementStage::Final,
            "final physical rect or DPI mismatch",
        ));
    }
    Ok(PlacementApplied { moved: true })
}

pub fn monitor_for_rect(rect: PhysicalRect) -> Option<MonitorFacts> {
    native::monitor_for_rect(rect, true)
}
pub fn monitors() -> Vec<MonitorFacts> {
    native::monitors()
}
pub fn borderless_target_for(intent: PhysicalRect) -> Option<PhysicalRect> {
    monitor_for_rect(intent).map(|m| m.rc_monitor)
}
pub fn read_outer_rect(hwnd: isize) -> Option<PhysicalRect> {
    native::Native.outer_rect(hwnd)
}
pub fn read_client_size(hwnd: isize) -> Option<[i32; 2]> {
    native::Native.client_size(hwnd)
}
pub fn read_zoomed(hwnd: isize) -> bool {
    native::Native.zoomed(hwnd)
}
pub fn read_normal_outer_rect(hwnd: isize) -> Option<PhysicalRect> {
    native::normal_outer_rect(hwnd)
}
pub fn monitor_for_window(hwnd: isize) -> Option<MonitorFacts> {
    native::Native.monitor_for_window(hwnd)
}

pub fn apply_outer_rect(
    hwnd: isize,
    target: PhysicalRect,
) -> Result<PlacementApplied, PlacementMismatch> {
    let result = apply_outer_rect_with(&mut native::Native, hwnd, target);
    match &result {
        Ok(applied) => crate::logger::log(format!(
            "[detached-placement] hwnd={hwnd:#x} target={target:?} moved={} outcome=Exact",
            applied.moved
        )),
        Err(error) => crate::logger::log(format!(
            "[detached-placement] hwnd={hwnd:#x} outcome=Mismatch {error:?}"
        )),
    }
    result
}

pub fn decorated_frame(dpi: u32) -> Option<FrameInsets> {
    native::decorated_frame(dpi)
}

/// Native logical coordinates, before the UI zoom conversion in the caller.
pub fn provisional_builder_geometry(
    placement: PhysicalWindowPlacement,
    decorated: bool,
) -> Option<(egui::Pos2, egui::Vec2)> {
    let monitor = monitor_for_rect(placement.outer)?;
    let frame = if decorated {
        decorated_frame(monitor.dpi)?
    } else {
        FrameInsets::default()
    };
    let seed = builder_placement(placement, monitor, frame, 1.0);
    Some((
        egui::pos2(seed.position[0], seed.position[1]),
        egui::vec2(seed.inner_size[0], seed.inner_size[1]),
    ))
}

#[cfg(windows)]
mod native {
    use super::*;
    use windows::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST,
        MONITOR_DEFAULTTONULL, MONITORINFO, MonitorFromRect, MonitorFromWindow,
    };
    use windows::Win32::UI::HiDpi::{
        AdjustWindowRectExForDpi, GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClientRect, GetWindowRect, IsZoomed, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
        SetWindowPos, WS_EX_WINDOWEDGE, WS_OVERLAPPEDWINDOW,
    };
    use windows::core::BOOL;

    fn rect(r: RECT) -> PhysicalRect {
        PhysicalRect {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        }
    }
    fn winrect(r: PhysicalRect) -> RECT {
        RECT {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        }
    }
    fn facts(handle: HMONITOR) -> Option<MonitorFacts> {
        if handle.0.is_null() {
            return None;
        }
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !unsafe { GetMonitorInfoW(handle, &mut info) }.as_bool() {
            return None;
        }
        let (mut x, mut y) = (0, 0);
        unsafe { GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &mut x, &mut y) }.ok()?;
        (x > 0).then_some(MonitorFacts {
            id: handle.0 as isize,
            rc_monitor: rect(info.rcMonitor),
            rc_work: rect(info.rcWork),
            dpi: x,
        })
    }
    pub(super) fn monitor_for_rect(r: PhysicalRect, nearest: bool) -> Option<MonitorFacts> {
        facts(unsafe {
            MonitorFromRect(
                &winrect(r),
                if nearest {
                    MONITOR_DEFAULTTONEAREST
                } else {
                    MONITOR_DEFAULTTONULL
                },
            )
        })
    }
    pub(super) fn monitors() -> Vec<MonitorFacts> {
        unsafe extern "system" fn collect(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
            if let Some(m) = facts(m) {
                unsafe { &mut *(data.0 as *mut Vec<MonitorFacts>) }.push(m);
            }
            BOOL(1)
        }
        let mut result = Vec::new();
        let ok = unsafe {
            EnumDisplayMonitors(
                None,
                None,
                Some(collect),
                LPARAM(&mut result as *mut _ as isize),
            )
        };
        if !ok.as_bool() {
            result.clear();
        }
        result
    }
    pub(super) fn decorated_frame(dpi: u32) -> Option<FrameInsets> {
        let mut r = RECT::default();
        // winit's ordinary resizable top-level style includes WS_EX_WINDOWEDGE.
        unsafe {
            AdjustWindowRectExForDpi(&mut r, WS_OVERLAPPEDWINDOW, false, WS_EX_WINDOWEDGE, dpi)
        }
        .ok()?;
        Some(FrameInsets {
            width: r.right - r.left,
            height: r.bottom - r.top,
        })
    }
    pub(super) fn normal_outer_rect(hwnd: isize) -> Option<PhysicalRect> {
        use windows::Win32::UI::WindowsAndMessaging::{
            GWL_EXSTYLE, GetWindowLongPtrW, GetWindowPlacement, WINDOWPLACEMENT, WS_EX_TOOLWINDOW,
        };
        let hwnd = HWND(hwnd as *mut _);
        let mut placement = WINDOWPLACEMENT {
            length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        unsafe { GetWindowPlacement(hwnd, &mut placement) }.ok()?;
        let normal = rect(placement.rcNormalPosition);
        if unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32 & WS_EX_TOOLWINDOW.0 != 0 {
            return normal.is_valid().then_some(normal);
        }
        normal_screen_rect(normal, &monitors())
    }
    pub(super) struct Native;
    impl WindowGeometry for Native {
        fn monitor_for_rect(&self, r: PhysicalRect) -> Option<MonitorFacts> {
            monitor_for_rect(r, true)
        }
        fn outer_rect(&self, hwnd: isize) -> Option<PhysicalRect> {
            let mut r = RECT::default();
            unsafe { GetWindowRect(HWND(hwnd as *mut _), &mut r) }.ok()?;
            Some(rect(r))
        }
        fn client_size(&self, hwnd: isize) -> Option<[i32; 2]> {
            let mut r = RECT::default();
            unsafe { GetClientRect(HWND(hwnd as *mut _), &mut r) }.ok()?;
            Some([r.right - r.left, r.bottom - r.top])
        }
        fn dpi(&self, hwnd: isize) -> Option<u32> {
            let dpi = unsafe { GetDpiForWindow(HWND(hwnd as *mut _)) };
            (dpi > 0).then_some(dpi)
        }
        fn monitor_for_window(&self, hwnd: isize) -> Option<MonitorFacts> {
            facts(unsafe { MonitorFromWindow(HWND(hwnd as *mut _), MONITOR_DEFAULTTONEAREST) })
        }
        fn zoomed(&self, hwnd: isize) -> bool {
            unsafe { IsZoomed(HWND(hwnd as *mut _)) }.as_bool()
        }
        fn set_outer_rect(&mut self, hwnd: isize, r: PhysicalRect, move_only: bool) -> bool {
            let mut flags = SWP_NOACTIVATE | SWP_NOZORDER;
            if move_only {
                flags |= SWP_NOSIZE;
            }
            unsafe {
                SetWindowPos(
                    HWND(hwnd as *mut _),
                    None,
                    r.left,
                    r.top,
                    r.width(),
                    r.height(),
                    flags,
                )
            }
            .is_ok()
        }
    }
}

#[cfg(not(windows))]
mod native {
    use super::*;
    pub(super) fn monitor_for_rect(_: PhysicalRect, _: bool) -> Option<MonitorFacts> {
        None
    }
    pub(super) fn monitors() -> Vec<MonitorFacts> {
        Vec::new()
    }
    pub(super) fn decorated_frame(_: u32) -> Option<FrameInsets> {
        None
    }
    pub(super) fn normal_outer_rect(_: isize) -> Option<PhysicalRect> {
        None
    }
    pub(super) struct Native;
    impl WindowGeometry for Native {
        fn monitor_for_rect(&self, _: PhysicalRect) -> Option<MonitorFacts> {
            None
        }
        fn outer_rect(&self, _: isize) -> Option<PhysicalRect> {
            None
        }
        fn client_size(&self, _: isize) -> Option<[i32; 2]> {
            None
        }
        fn dpi(&self, _: isize) -> Option<u32> {
            None
        }
        fn monitor_for_window(&self, _: isize) -> Option<MonitorFacts> {
            None
        }
        fn zoomed(&self, _: isize) -> bool {
            false
        }
        fn set_outer_rect(&mut self, _: isize, _: PhysicalRect, _: bool) -> bool {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> PhysicalRect {
        PhysicalRect {
            left: x,
            top: y,
            right: x + w,
            bottom: y + h,
        }
    }
    fn monitor(id: isize, r: PhysicalRect, dpi: u32) -> MonitorFacts {
        MonitorFacts {
            id,
            rc_monitor: r,
            rc_work: PhysicalRect {
                bottom: r.bottom - 40,
                ..r
            },
            dpi,
        }
    }
    fn desktop() -> Vec<MonitorFacts> {
        vec![
            monitor(1, rect(0, 0, 3840, 2160), 144),
            monitor(2, rect(6000, 1500, 2560, 1440), 96),
        ]
    }
    fn frame(dpi: u32, decorated: bool) -> FrameInsets {
        if decorated {
            FrameInsets {
                width: (16 * dpi / 96) as i32,
                height: (40 * dpi / 96) as i32,
            }
        } else {
            FrameInsets::default()
        }
    }

    /// Selection mirrors MonitorFromRect: maximum intersection, nearest if none.
    fn select(monitors: &[MonitorFacts], r: PhysicalRect, nearest: bool) -> Option<MonitorFacts> {
        let best = monitors
            .iter()
            .max_by_key(|m| r.intersection_area(m.rc_monitor))
            .copied()?;
        if r.intersection_area(best.rc_monitor) > 0 {
            return Some(best);
        }
        if !nearest {
            return None;
        }
        monitors
            .iter()
            .min_by_key(|m| {
                let x = (i64::from(m.rc_monitor.left) - i64::from(r.right))
                    .max(0)
                    .max(i64::from(r.left) - i64::from(m.rc_monitor.right));
                let y = (i64::from(m.rc_monitor.top) - i64::from(r.bottom))
                    .max(0)
                    .max(i64::from(r.top) - i64::from(m.rc_monitor.bottom));
                x * x + y * y
            })
            .copied()
    }

    struct Fake {
        monitors: Vec<MonitorFacts>,
        outer: PhysicalRect,
        client: [i32; 2],
        dpi: u32,
        decorated: bool,
        calls: Vec<(PhysicalRect, bool)>,
        zoomed: bool,
        fail_on: Option<usize>,
        wrong_dpi: bool,
    }
    impl Fake {
        fn new(monitors: Vec<MonitorFacts>, outer: PhysicalRect, decorated: bool) -> Self {
            let dpi = select(&monitors, outer, true).unwrap().dpi;
            let f = frame(dpi, decorated);
            Self {
                monitors,
                outer,
                client: [outer.width() - f.width, outer.height() - f.height],
                dpi,
                decorated,
                calls: vec![],
                zoomed: false,
                fail_on: None,
                wrong_dpi: false,
            }
        }
        fn rebuild_for_dpi(
            &self,
            requested: PhysicalRect,
            new_monitor: MonitorFacts,
        ) -> (PhysicalRect, [i32; 2]) {
            // winit reads the CURRENT client, not the requested SetWindowPos size.
            let ratio = f64::from(new_monitor.dpi) / f64::from(self.dpi);
            let client = [
                (f64::from(self.client[0]) * ratio).round() as i32,
                (f64::from(self.client[1]) * ratio).round() as i32,
            ];
            let f = frame(new_monitor.dpi, self.decorated);
            let mut conservative = rect(
                requested.left,
                requested.top,
                client[0] + f.width,
                client[1] + f.height,
            );
            let wrong = select(&self.monitors, conservative, false);
            if wrong.map(|m| m.id) != Some(new_monitor.id) {
                let wrong_rect = wrong.map(|m| m.rc_monitor).unwrap_or_default();
                let new = new_monitor.rc_monitor;
                // Same shared-edge conditions and width+height bound as winit
                // 0.30.13 event_loop.rs WM_DPICHANGED. Never blanket-clamp.
                let dx = if wrong_rect.left == new.right {
                    -1
                } else if wrong_rect.right == new.left {
                    1
                } else {
                    0
                };
                let dy = if wrong_rect.bottom == new.top {
                    1
                } else if wrong_rect.top == new.bottom {
                    -1
                } else {
                    0
                };
                for _ in 0..new.width() + new.height() {
                    conservative = conservative.translated(dx, dy);
                    if select(&self.monitors, conservative, false).map(|m| m.id)
                        == Some(new_monitor.id)
                    {
                        break;
                    }
                }
            }
            (conservative, client)
        }
    }
    impl WindowGeometry for Fake {
        fn monitor_for_rect(&self, r: PhysicalRect) -> Option<MonitorFacts> {
            select(&self.monitors, r, true)
        }
        fn outer_rect(&self, _: isize) -> Option<PhysicalRect> {
            Some(self.outer)
        }
        fn client_size(&self, _: isize) -> Option<[i32; 2]> {
            Some(self.client)
        }
        fn dpi(&self, _: isize) -> Option<u32> {
            Some(self.dpi)
        }
        fn monitor_for_window(&self, _: isize) -> Option<MonitorFacts> {
            self.monitor_for_rect(self.outer)
        }
        fn zoomed(&self, _: isize) -> bool {
            self.zoomed
        }
        fn set_outer_rect(&mut self, _: isize, requested: PhysicalRect, move_only: bool) -> bool {
            self.calls.push((requested, move_only));
            if self.fail_on == Some(self.calls.len()) {
                return false;
            }
            let requested = if move_only {
                rect(
                    requested.left,
                    requested.top,
                    self.outer.width(),
                    self.outer.height(),
                )
            } else {
                requested
            };
            let monitor = self.monitor_for_rect(requested).unwrap();
            if monitor.dpi != self.dpi {
                (self.outer, self.client) = self.rebuild_for_dpi(requested, monitor);
                if !self.wrong_dpi {
                    self.dpi = monitor.dpi;
                }
            } else {
                self.outer = requested;
                let f = frame(self.dpi, self.decorated);
                self.client = [requested.width() - f.width, requested.height() - f.height];
            }
            true
        }
    }

    #[test]
    fn physical_placement_serde_round_trip_and_invalid_rect() {
        let p = PhysicalWindowPlacement {
            outer: rect(-6000, -200, 1600, 900),
            maximized: true,
        };
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(
            serde_json::from_str::<PhysicalWindowPlacement>(&json).unwrap(),
            p
        );
        assert!(
            !PhysicalWindowPlacement {
                outer: rect(0, 0, -1, 900),
                ..p
            }
            .is_sane()
        );
        assert!(
            !PhysicalRect {
                left: i32::MIN,
                top: 0,
                right: i32::MAX,
                bottom: 900
            }
            .is_valid()
        );
    }

    #[test]
    fn defaults_offsets_and_builder_keep_monitor_scale_and_ui_zoom_separate() {
        let m = desktop()[0];
        let f = frame(m.dpi, true);
        let p = default_placement(rect(100, 200, 1000, 700), m, f);
        assert_eq!(p.outer, rect(172, 272, 1440 + f.width, 1080 + f.height));
        let offset = offset_placement(
            PhysicalWindowPlacement {
                maximized: true,
                ..p
            },
            2,
            m,
        );
        assert_eq!(offset.outer, p.outer.translated(120, 120));
        assert!(!offset.maximized);
        for dpi in [96, 144] {
            for decorated in [false, true] {
                for ui in [1.0, 1.25, 2.0] {
                    let m = MonitorFacts { dpi, ..m };
                    let f = frame(dpi, decorated);
                    let s = m.scale();
                    let p = PhysicalWindowPlacement {
                        outer: rect(
                            (200.0 * s) as i32,
                            (100.0 * s) as i32,
                            (960.0 * s) as i32 + f.width,
                            (720.0 * s) as i32 + f.height,
                        ),
                        maximized: false,
                    };
                    let b = builder_placement(p, m, f, ui);
                    assert_eq!(b.position, [200.0 / ui, 100.0 / ui]);
                    assert_eq!(b.inner_size, [960.0 / ui, 720.0 / ui]);
                }
            }
        }
    }

    #[test]
    fn legacy_migration_selects_each_monitor_and_rejects_removed_monitor() {
        let ms = desktop();
        let fs = ms.iter().map(|m| frame(m.dpi, true)).collect::<Vec<_>>();
        for (legacy, expected) in [
            (
                DetachedViewerWindowPlacement {
                    x: 200.0,
                    y: 100.0,
                    w: 960.0,
                    h: 720.0,
                    maximized: true,
                },
                rect(300, 150, 1440 + fs[0].width, 1080 + fs[0].height),
            ),
            (
                DetachedViewerWindowPlacement {
                    x: 6100.0,
                    y: 1600.0,
                    w: 960.0,
                    h: 720.0,
                    maximized: false,
                },
                rect(6100, 1600, 960 + fs[1].width, 720 + fs[1].height),
            ),
        ] {
            let migrated = migrate_legacy_placement(legacy, &ms, &fs).unwrap();
            assert_eq!(migrated.outer, expected);
            assert_eq!(migrated.maximized, legacy.maximized);
        }
        let missing = DetachedViewerWindowPlacement {
            x: -9000.0,
            y: 0.0,
            w: 960.0,
            h: 720.0,
            maximized: false,
        };
        assert!(migrate_legacy_placement(missing, &ms, &fs).is_none());
        let overlapping = [
            monitor(1, rect(0, 0, 1920, 1080), 96),
            monitor(2, rect(1920, 0, 2560, 1440), 144),
        ];
        let ambiguous = DetachedViewerWindowPlacement {
            x: 1100.0,
            y: 0.0,
            w: 1000.0,
            h: 700.0,
            maximized: false,
        };
        // Both scaled centers fit; the 150% candidate has greater overlap.
        assert_eq!(
            migrate_legacy_placement(ambiguous, &overlapping, &[FrameInsets::default(); 2])
                .unwrap()
                .outer
                .left,
            1650
        );
    }

    #[test]
    fn workspace_normal_rect_is_converted_using_its_monitor() {
        let mut ms = desktop();
        ms[1].rc_work.left += 48;
        ms[1].rc_work.top += 30;
        let normal = rect(6200 - 48, 1700 - 30, 960, 720);
        assert_eq!(
            normal_screen_rect(normal, &ms),
            Some(rect(6200, 1700, 960, 720))
        );
    }

    #[test]
    fn same_dpi_exact_does_not_move_and_same_dpi_change_has_no_staging() {
        let initial = rect(100, 100, 1400, 900);
        let mut fake = Fake::new(desktop(), initial, true);
        assert_eq!(
            apply_outer_rect_with(&mut fake, 1, initial),
            Ok(PlacementApplied { moved: false })
        );
        assert!(fake.calls.is_empty());
        let target = rect(-300, 200, 3000, 1700);
        assert!(apply_outer_rect_with(&mut fake, 1, target).is_ok());
        assert_eq!(fake.calls, vec![(target, false)]);
    }

    #[test]
    fn cross_dpi_stages_current_client_before_move_and_places_exactly() {
        for decorated in [false, true] {
            for vertical in [false, true] {
                let ms = if vertical {
                    vec![
                        monitor(1, rect(0, 0, 3840, 2160), 144),
                        monitor(2, rect(0, -1440, 2560, 1440), 96),
                    ]
                } else {
                    desktop()
                };
                for from in 0..2 {
                    let to = 1 - from;
                    let source = ms[from];
                    let dest = ms[to];
                    for target in [
                        centered(dest.rc_work, 1000, 700),
                        rect(
                            dest.rc_monitor.left - 300,
                            dest.rc_monitor.top + 100,
                            1400,
                            850,
                        ),
                        centered(
                            dest.rc_monitor,
                            dest.rc_monitor.width() + 800,
                            dest.rc_monitor.height() + 600,
                        ),
                    ] {
                        let initial = centered(source.rc_work, 1800, 1000);
                        let mut fake = Fake::new(ms.clone(), initial, decorated);
                        assert_eq!(
                            apply_outer_rect_with(&mut fake, 1, target),
                            Ok(PlacementApplied { moved: true }),
                            "decorated={decorated} vertical={vertical} from={from} target={target:?}"
                        );
                        assert_eq!(fake.outer, target);
                        assert_eq!(fake.dpi, select(&ms, target, true).unwrap().dpi);
                        assert_eq!(fake.calls.len(), 3);
                        assert!(!fake.calls[0].1);
                        assert!(fake.calls[1].1);
                        assert!(!fake.calls[2].1);
                        let stage = fake.calls[0].0;
                        let f = frame(source.dpi, decorated);
                        assert!(stage.width() - f.width <= 320 && stage.height() - f.height <= 240);
                    }
                }
            }
        }
    }

    #[test]
    fn fake_rebuild_uses_current_client_and_does_not_clamp_disconnected_monitors() {
        let mut fake = Fake::new(desktop(), rect(0, 0, 3000, 1800), false);
        let requested = rect(6100, 1600, 20, 20);
        fake.set_outer_rect(1, requested, false);
        assert_eq!(fake.client, [2000, 1200]);
        assert_eq!(fake.outer.width(), 2000);
        // Without staging, a very large current client need not stay on the new
        // monitor; the fake deliberately does not force successful membership.
        let fake = Fake::new(desktop(), rect(6200, 1600, 3200, 1800), false);
        let (r, _) = fake.rebuild_for_dpi(rect(3000, 1600, 20, 20), desktop()[0]);
        assert_eq!(r.left, 3000);
        assert_eq!(
            select(&fake.monitors, r, false).unwrap().id,
            2,
            "disconnected edges must not acquire an artificial nudge/clamp"
        );
    }

    #[test]
    fn mismatch_reports_stage_rect_dpi_and_never_proceeds_after_failed_staging() {
        for (fail_on, expected_stage) in [
            (1, PlacementStage::ShrinkAtCurrentDpi),
            (2, PlacementStage::MoveToTargetDpi),
            (3, PlacementStage::Final),
        ] {
            let mut fake = Fake::new(desktop(), rect(100, 100, 1600, 1000), true);
            fake.fail_on = Some(fail_on);
            let target = rect(6200, 1600, 1000, 700);
            let failure = apply_outer_rect_with(&mut fake, 1, target).unwrap_err();
            assert_eq!(failure.stage, expected_stage);
            assert_eq!(failure.target, target);
            assert_eq!(failure.expected_dpi, Some(96));
            assert!(failure.actual.is_some());
            assert_eq!(fake.calls.len(), fail_on);
        }
        let mut fake = Fake::new(desktop(), rect(100, 100, 1600, 1000), false);
        fake.wrong_dpi = true;
        assert_eq!(
            apply_outer_rect_with(&mut fake, 1, rect(6200, 1600, 1000, 700))
                .unwrap_err()
                .stage,
            PlacementStage::MoveToTargetDpi
        );
        assert_eq!(fake.calls.len(), 2);
        fake.zoomed = true;
        fake.calls.clear();
        assert_eq!(
            apply_outer_rect_with(&mut fake, 1, rect(100, 100, 1000, 700))
                .unwrap_err()
                .stage,
            PlacementStage::Preconditions
        );
        assert!(fake.calls.is_empty());
    }

    /// No application, profile, renderer, visibility, focus or input is involved.
    /// One test owns the sole winit event loop allowed in this test process.
    #[cfg(windows)]
    #[test]
    fn hidden_winit_hwnd_physical_placement_and_available_mixed_dpi_monitors() {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::System::Threading::GetCurrentProcess;
        use windows::Win32::UI::HiDpi::{
            AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            DPI_AWARENESS_PER_MONITOR_AWARE, GetAwarenessFromDpiAwarenessContext,
            GetDpiAwarenessContextForProcess, GetDpiForSystem, GetProcessDpiAwareness,
            GetThreadDpiAwarenessContext, GetWindowDpiAwarenessContext,
            PROCESS_PER_MONITOR_DPI_AWARE,
        };
        use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;
        use winit::event_loop::EventLoop;
        use winit::platform::windows::EventLoopBuilderExtWindows;
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use winit::window::Window;

        // winit tries PMv2 and PMv1 once, but deliberately ignores failure when
        // the process awareness was already fixed. GetDpiForMonitor then returns
        // the system DPI for EVERY monitor in a system-aware process, and an
        // equally system-aware HWND makes the old rect/DPI assertions falsely
        // pass. Validate this premise before deciding mixed-DPI coverage can skip.
        let diagnose_awareness = |stage: &str| {
            let process_context = unsafe { GetDpiAwarenessContextForProcess(GetCurrentProcess()) };
            let thread_context = unsafe { GetThreadDpiAwarenessContext() };
            let process_awareness = unsafe { GetProcessDpiAwareness(None) }
                .expect("query hidden-test process DPI awareness");
            let thread_awareness = unsafe { GetAwarenessFromDpiAwarenessContext(thread_context) };
            eprintln!(
                "hidden winit DPI awareness stage={stage} process_context={process_context:?} process_awareness={process_awareness:?} thread_context={thread_context:?} thread_awareness={thread_awareness:?} process_pmv2={} thread_pmv2={} system_dpi={}",
                unsafe {
                    AreDpiAwarenessContextsEqual(
                        process_context,
                        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                    )
                }
                .as_bool(),
                unsafe {
                    AreDpiAwarenessContextsEqual(
                        thread_context,
                        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                    )
                }
                .as_bool(),
                unsafe { GetDpiForSystem() }
            );
            (process_awareness, thread_awareness)
        };
        diagnose_awareness("before_event_loop");
        let mut builder = EventLoop::builder();
        builder.with_any_thread(true);
        let event_loop = builder.build().expect("hidden winit event loop");
        let (process_awareness, thread_awareness) = diagnose_awareness("after_event_loop");
        assert_eq!(
            process_awareness, PROCESS_PER_MONITOR_DPI_AWARE,
            "GetDpiForMonitor cannot establish mixed-DPI coverage in a non-per-monitor-aware process; do not count this as a valid single-DPI skip"
        );
        assert_eq!(
            thread_awareness, DPI_AWARENESS_PER_MONITOR_AWARE,
            "the hidden HWND must be created on a per-monitor-aware thread; do not count an awareness failure as mixed-DPI skip"
        );
        let ms = monitors();
        if ms.is_empty() {
            eprintln!("SKIP hidden winit HWND: no monitor available");
            return;
        }
        eprintln!("hidden winit monitor facts under per-monitor awareness: {ms:?}");
        let mixed = ms.iter().any(|m| ms.iter().any(|other| other.dpi != m.dpi));
        let mut observed_dpis = std::collections::BTreeSet::new();
        for decorated in [true, false] {
            #[allow(deprecated)]
            let window = event_loop
                .create_window(
                    Window::default_attributes()
                        .with_visible(false)
                        .with_decorations(decorated)
                        .with_title("mIV hidden geometry unit test"),
                )
                .expect("hidden winit window");
            let RawWindowHandle::Win32(handle) = window.window_handle().unwrap().as_raw() else {
                panic!("Win32 handle expected")
            };
            let hwnd = handle.hwnd.get();
            let window_context = unsafe { GetWindowDpiAwarenessContext(HWND(hwnd as *mut _)) };
            let window_awareness = unsafe { GetAwarenessFromDpiAwarenessContext(window_context) };
            eprintln!(
                "hidden winit decorated={decorated} hwnd={hwnd:#x} window_context={window_context:?} window_awareness={window_awareness:?} window_pmv2={}",
                unsafe {
                    AreDpiAwarenessContextsEqual(
                        window_context,
                        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                    )
                }
                .as_bool()
            );
            assert_eq!(
                window_awareness, DPI_AWARENESS_PER_MONITOR_AWARE,
                "a system-aware/unaware HWND would hide real monitor DPI differences"
            );
            assert!(!unsafe { IsWindowVisible(HWND(hwnd as *mut _)) }.as_bool());
            for source in &ms {
                let start = centered(
                    source.rc_work,
                    source.rc_work.width().min(900),
                    source.rc_work.height().min(600),
                );
                apply_outer_rect(hwnd, start).expect("source placement must be Exact");
                let window_dpi = native::Native
                    .dpi(hwnd)
                    .expect("real HWND DPI at source monitor");
                let actual_monitor = monitor_for_window(hwnd).expect("real HWND source monitor");
                eprintln!(
                    "hidden winit source probe decorated={decorated} monitor_id={:#x} facts_dpi={} hwnd_dpi={window_dpi} outer={:?}",
                    source.id,
                    source.dpi,
                    read_outer_rect(hwnd)
                );
                assert_eq!(
                    actual_monitor.id, source.id,
                    "source probe must be on its intended monitor"
                );
                assert_eq!(
                    window_dpi, source.dpi,
                    "GetDpiForMonitor facts must agree with the per-monitor-aware HWND"
                );
                observed_dpis.insert(window_dpi);
                let client = native::Native.client_size(hwnd).expect("real client size");
                let expected_frame = if decorated {
                    decorated_frame(source.dpi).unwrap()
                } else {
                    FrameInsets::default()
                };
                assert_eq!(
                    [start.width() - client[0], start.height() - client[1]],
                    [expected_frame.width, expected_frame.height],
                    "builder seed extents must match winit decorations"
                );
                for dest in &ms {
                    let targets = [
                        centered(
                            dest.rc_work,
                            dest.rc_work.width().min(1000),
                            dest.rc_work.height().min(700),
                        ),
                        rect(
                            dest.rc_monitor.left - 200,
                            dest.rc_monitor.top + 80,
                            dest.rc_monitor.width().min(1200),
                            dest.rc_monitor.height().min(800),
                        ),
                        centered(
                            dest.rc_monitor,
                            dest.rc_monitor.width() + 600,
                            dest.rc_monitor.height() + 400,
                        ),
                    ];
                    for target in targets {
                        apply_outer_rect(hwnd, start).expect("reset source must be Exact");
                        let result = apply_outer_rect(hwnd, target);
                        eprintln!(
                            "hidden winit decorated={decorated} source_dpi={} target_dpi={} target={target:?} result={result:?}",
                            source.dpi,
                            monitor_for_rect(target).unwrap().dpi
                        );
                        result.expect("real HWND placement must be Exact");
                        assert_eq!(read_outer_rect(hwnd), Some(target));
                        assert_eq!(
                            native::Native.dpi(hwnd),
                            monitor_for_rect(target).map(|m| m.dpi)
                        );
                        assert!(!unsafe { IsWindowVisible(HWND(hwnd as *mut _)) }.as_bool());
                    }
                }
            }
        }
        assert_eq!(
            mixed,
            observed_dpis.len() > 1,
            "mixed-DPI coverage must agree with independently observed per-monitor-aware HWND DPIs"
        );
        if !mixed {
            eprintln!(
                "SKIP mixed-DPI real-monitor coverage: validated per-monitor-aware process/thread/HWNDs observed only {observed_dpis:?} on {} monitors; single-DPI HWND coverage passed",
                ms.len()
            );
        }
    }
}
