use crate::ErrorCode;
use mimageviewer_browser_view_ipc::Placement;
use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub screen_x: f64,
    pub screen_y: f64,
    pub outer_width: f64,
    pub outer_height: f64,
    pub inner_width: f64,
    pub inner_height: f64,
    pub device_pixel_ratio: f64,
    #[serde(default, rename = "context_event")]
    pub context_event: Option<ContextEvent>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextEvent {
    pub screen_x: f64,
    pub screen_y: f64,
    pub client_x: f64,
    pub client_y: f64,
    #[serde(rename = "win_screenX")]
    pub window_screen_x: f64,
    #[serde(rename = "win_screenY")]
    pub window_screen_y: f64,
    pub inner_width: f64,
    pub inner_height: f64,
    pub device_pixel_ratio: f64,
}
#[derive(Clone, Deserialize, Serialize)]
pub struct BrowserWindow {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
    pub state: String,
}
fn finite(values: &[f64]) -> bool {
    values.iter().all(|value| value.is_finite())
}
fn positive(values: &[f64]) -> bool {
    finite(values) && values.iter().all(|value| *value > 0.0)
}

pub fn calculate(view: &View, zoom: f64, window: &BrowserWindow) -> Result<Placement, ErrorCode> {
    let bad = ErrorCode::InvalidPlacement;
    if !finite(&[view.screen_x, view.screen_y, window.left, window.top])
        || !positive(&[
            zoom,
            view.device_pixel_ratio,
            view.inner_width,
            view.inner_height,
            view.outer_width,
            view.outer_height,
            window.width,
            window.height,
        ])
        || !matches!(
            window.state.as_str(),
            "normal" | "maximized" | "fullscreen" | "minimized"
        )
    {
        return Err(bad);
    }
    let width = view.inner_width * zoom;
    let height = view.inner_height * zoom;
    let scale = view.device_pixel_ratio / zoom;
    let mut origin = (
        view.screen_x + view.outer_width - 8.0 - width,
        view.screen_y + view.outer_height - 8.0 - height,
    );
    if let Some(event) = &view.context_event {
        if !finite(&[
            event.screen_x,
            event.screen_y,
            event.client_x,
            event.client_y,
            event.window_screen_x,
            event.window_screen_y,
        ]) || !positive(&[
            event.inner_width,
            event.inner_height,
            event.device_pixel_ratio,
        ]) {
            return Err(bad);
        }
        // Freshness is geometry identity, never a clock window or page/element content.
        if event.window_screen_x == view.screen_x
            && event.window_screen_y == view.screen_y
            && event.inner_width == view.inner_width
            && event.inner_height == view.inner_height
            && event.device_pixel_ratio == view.device_pixel_ratio
        {
            origin = (
                event.screen_x - event.client_x * zoom,
                event.screen_y - event.client_y * zoom,
            );
        }
    }
    let coordinate = |value: f64| -> Result<i32, ErrorCode> {
        let value = value.round();
        if value.is_finite() && value >= i32::MIN as f64 && value <= i32::MAX as f64 {
            Ok(value as i32)
        } else {
            Err(bad)
        }
    };
    let placement = Placement {
        left: coordinate(origin.0 * scale)?,
        top: coordinate(origin.1 * scale)?,
        right: coordinate((origin.0 + width) * scale)?,
        bottom: coordinate((origin.1 + height) * scale)?,
        // This is the browser's content rectangle, even when the browser itself
        // is maximized. Maximizing mIV would replace it with the monitor work area.
        maximized: false,
    };
    placement.validate().map_err(|_| bad)?;
    Ok(placement)
}
