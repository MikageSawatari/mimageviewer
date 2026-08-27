use std::collections::{HashMap, VecDeque};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use serde_json::Value;
use windows::Win32::Foundation::{CloseHandle, HANDLE, RECT, WAIT_TIMEOUT};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_FLAG,
    D3D11_FENCE_FLAG_NONE, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11Device1,
    ID3D11Device5, ID3D11DeviceContext, ID3D11DeviceContext1, ID3D11DeviceContext4, ID3D11Fence,
    ID3D11RenderTargetView, ID3D11Resource, ID3D11Texture2D, ID3D11View,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM,
    DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
    DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT, DXGI_SWAP_EFFECT_FLIP_DISCARD,
    DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIAdapter, IDXGIAdapter1, IDXGIDevice, IDXGIFactory2,
    IDXGIKeyedMutex, IDXGIOutput, IDXGISwapChain1, IDXGISwapChain2,
};
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::core::Interface;
use windows_numerics::Matrix3x2;

use crate::panorama::{PanoPose, PanoUvTransform};
use crate::settings::{FsSidePanelMode, VideoBottomLock, VideoScaleFilter};
use crate::ui_helpers::HoverTipExt;
use crate::video::decoder::{VideoFrame, VideoFrameData};
use crate::video::display_metadata::VideoOrientation;
pub(crate) use crate::video::native_cursor::cursor_move_is_activity;
use crate::video::native_window_health::NativeRenderOperation;
use crate::video::native_window_host::{
    NativeCursorIcon, NativeRenderTarget, NativeRenderTargets, NativeWindowIntent,
    NativeWindowObservation,
};
use crate::video::window_host_contract::HostWindowTopology;
use crate::video::{VideoAnime4kFallbackNotice, VideoScaleFallbackNotice};

// 音楽ビュー (Inc 5c-A) がジャンプ/ブックマークパネル本体・一括登録ダイアログを共有するため
// crate 内に公開する。動画専用の `pub(super)` ヘルパは従来どおり parent 限定のまま。
use super::overlay_draw::*;
use super::overlay_gpu::{DeviceEpoch, overlay_gpu_service};

#[path = "grade_pipeline.rs"]
mod grade_pipeline;
use self::grade_pipeline::VideoGradePipeline;
#[path = "panorama_pipeline.rs"]
mod panorama_pipeline;
use self::panorama_pipeline::VideoPanoramaPipeline;
#[path = "resample_pipeline.rs"]
mod resample_pipeline;
use self::resample_pipeline::{
    VideoResampleMode, VideoResamplePipeline, VideoResamplePrepareError,
    measure_video_anime4k_on_adapter, select_video_resample_mode,
};
#[path = "surface_policy.rs"]
mod surface_policy;
use self::surface_policy::{
    VideoScaleFallbackReason, VideoSurfaceSizeDecision, VideoSurfaceSizeInput,
    decide_video_surface_size,
};

/// `shared_texture_cache` (presenter 側 `OpenSharedResource1` キャッシュ) の上限。
///
/// 2026-05-15 (Codex 助言): 旧 64 → 8 に縮小、SwitchSource ハンドラで `.clear()` も
/// 追加。cap=64 は `OpenSharedResource1` の再呼び出しを減らすためだが、各エントリは
/// D3D11 共有 texture を保持して adapter memory を圧迫していた (4K で 32 MB/枚)。
/// 単一動画の通常再生では 1-3 個の slot が周回するのみで cap=8 で十分。動画切替
/// (SwitchSource) ごとに clear して旧動画分のキャッシュを残さない。
const SHARED_TEXTURE_CACHE_CAPACITY: usize = 8;

const SEEK_STATUS_DELAY: Duration = Duration::from_millis(150);
const SEEK_STATUS_MIN_VISIBLE: Duration = Duration::from_millis(300);
const LIMITER_INDICATOR_VISIBLE: Duration = Duration::from_millis(500);
const TEXT_INPUT_FOCUS_CLAIM_MIN_INTERVAL: Duration = Duration::from_millis(500);
/// A geometry stream is considered settled after this long without a newer
/// `GeometryChanged` sample. Windows has no single reliable end edge across
/// interactive, child, maximize, and programmatic resize routes.
const VIDEO_RESIZE_SETTLE_INTERVAL: Duration = Duration::from_millis(150);

#[derive(Debug)]
pub enum NativeAnime4kMeasurementUpdate {
    Progress {
        completed: u32,
        point: crate::video::anime4k_policy::VideoAnime4kMeasurementPoint,
    },
    Completed(Result<crate::video::anime4k_policy::VideoAnime4kMeasurementCache, String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativeVideoAnime4kStatus {
    Waiting,
    Measuring {
        completed: u32,
        total: u32,
    },
    Active {
        variant: crate::video::anime4k_policy::VideoAnime4kVariant,
        predicted_us: u64,
        budget_us: Option<u64>,
    },
    Fallback(crate::video::anime4k_policy::VideoAnime4kFallbackReason),
    Failed,
}

pub struct NativeAnime4kMeasurement {
    pub updates: crossbeam_channel::Receiver<NativeAnime4kMeasurementUpdate>,
    cancel: Arc<AtomicBool>,
}

impl Drop for NativeAnime4kMeasurement {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

/// 動画 HUD 2 段化リデザイン (Phase 3): 下 HUD を **シーク行 (上段) + コントロール行 (下段)**
/// の 2 段に分割する。
///
/// - **シーク行** = `HUD_SEEK_ROW_HEIGHT` (24pt): seek bar + マーカー + hover サムネ trigger
/// - **コントロール行** = `HUD_CONTROLS_ROW_HEIGHT` (40pt): 再生/停止 / ループ / 音量等のボタン群
/// - **合計高さ** = `HUD_BOTTOM_HEIGHT` (= 上記の和、64pt)
///
/// 旧版は 46pt の 1 行構造で seek bar + コントロールが Y を共有していた。2 段化で
/// seek bar がフル幅 + ヒット領域 24pt に拡張され、4K/長尺動画の精密スクラブが楽になる。
/// activation zone (= cursor polling の下端帯) は HUD 高さと独立した 220pt 固定で touch しない
/// (詳細は `cursor_polling_tick` 周辺コメント参照)。
pub const HUD_SEEK_ROW_HEIGHT: f32 = 24.0;
pub const HUD_CONTROLS_ROW_HEIGHT: f32 = 40.0;
pub const HUD_BOTTOM_HEIGHT: f32 = HUD_SEEK_ROW_HEIGHT + HUD_CONTROLS_ROW_HEIGHT;
pub const HUD_TOP_HEIGHT: f32 = 54.0;
pub const SEEK_STRIP_HEIGHT: f32 = 104.0;
pub const SEEK_STRIP_CELL_WIDTH: f32 = 152.0;
const SEEK_STRIP_LOCK_BUTTON_SIZE: f32 = 28.0;
const SEEK_STRIP_RIGHT_INSET: f32 = 7.0;
const SEEK_STRIP_LOCK_TOP_INSET: f32 = 4.0;
const SEEK_STRIP_RANGE_LOCK_GAP: f32 = 6.0;
const SEEK_PREVIEW_GAP: f32 = 14.0;
const SEEK_PREVIEW_SCREEN_MARGIN: f32 = 8.0;
const SEEK_PREVIEW_ACTION_BAR_HEIGHT: f32 = 38.0;

fn native_seek_strip_rect(width: f32, height: f32) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(
            0.0,
            (height - HUD_BOTTOM_HEIGHT - SEEK_STRIP_HEIGHT).max(0.0),
        ),
        egui::vec2(width, SEEK_STRIP_HEIGHT.min(height.max(0.0))),
    )
}

fn native_seek_strip_lock_button_rect(strip_rect: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(
            strip_rect.max.x - SEEK_STRIP_RIGHT_INSET - SEEK_STRIP_LOCK_BUTTON_SIZE,
            strip_rect.min.y + SEEK_STRIP_LOCK_TOP_INSET,
        ),
        egui::vec2(SEEK_STRIP_LOCK_BUTTON_SIZE, SEEK_STRIP_LOCK_BUTTON_SIZE),
    )
}

fn native_seek_strip_range_text_pos(strip_rect: egui::Rect) -> egui::Pos2 {
    let lock_rect = native_seek_strip_lock_button_rect(strip_rect);
    egui::pos2(
        lock_rect.min.x - SEEK_STRIP_RANGE_LOCK_GAP,
        strip_rect.min.y + 5.0,
    )
}

fn seek_strip_body_accepts_pointer(lock_rect: egui::Rect, pointer: egui::Pos2) -> bool {
    !lock_rect.contains(pointer)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum NativeSeekStripPreviewHover {
    /// The pointer is not owned by the strip. The seek row or the existing preview corridor may
    /// still keep the single shared preview alive.
    Outside,
    /// The pointer is over strip chrome or an axis position without material. An old preview must
    /// not leak through from the seek row or a previous strip position.
    Suppress,
    Target(f64),
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct NativeSeekPreviewLayout {
    preview_rect: egui::Rect,
    image_rect: egui::Rect,
    action_rect: egui::Rect,
}

/// Place the one shared seek preview above the lowest visible timeline surface.
///
/// On a short viewport the image is reduced (keeping its 16:9 shape and the action row) before
/// applying the top margin. This preserves the stronger invariant that the preview never covers
/// the seek strip.
fn native_seek_preview_layout(
    overlay_width: f32,
    target_x: f32,
    hud_rect: egui::Rect,
    strip_rect: Option<egui::Rect>,
) -> NativeSeekPreviewLayout {
    let anchor_y = strip_rect.map_or(hud_rect.min.y, |rect| rect.min.y);
    let desired_image_width = (overlay_width * 0.30).clamp(300.0, 352.0);
    let horizontal_image_width = (overlay_width - SEEK_PREVIEW_SCREEN_MARGIN * 2.0).max(1.0);
    let available_total_height =
        (anchor_y - SEEK_PREVIEW_GAP - SEEK_PREVIEW_SCREEN_MARGIN).max(1.0);
    let vertical_image_width =
        ((available_total_height - SEEK_PREVIEW_ACTION_BAR_HEIGHT).max(1.0)) * (16.0 / 9.0);
    let image_width = desired_image_width
        .min(horizontal_image_width)
        .min(vertical_image_width);
    let image_size = egui::vec2(image_width, image_width * 9.0 / 16.0);
    let preview_size = egui::vec2(image_size.x, image_size.y + SEEK_PREVIEW_ACTION_BAR_HEIGHT);
    let preview_x = (target_x - preview_size.x * 0.5).clamp(
        SEEK_PREVIEW_SCREEN_MARGIN,
        (overlay_width - preview_size.x - SEEK_PREVIEW_SCREEN_MARGIN)
            .max(SEEK_PREVIEW_SCREEN_MARGIN),
    );
    let preview_y = (anchor_y - preview_size.y - SEEK_PREVIEW_GAP).max(SEEK_PREVIEW_SCREEN_MARGIN);
    let preview_rect = egui::Rect::from_min_size(egui::pos2(preview_x, preview_y), preview_size);
    let image_rect = egui::Rect::from_min_size(preview_rect.min, image_size);
    let action_rect = egui::Rect::from_min_max(
        egui::pos2(preview_rect.min.x, image_rect.max.y),
        preview_rect.max,
    );
    NativeSeekPreviewLayout {
        preview_rect,
        image_rect,
        action_rect,
    }
}

/// 共有プレビューの目的時刻と、**画面上のどこを指していたか** をまとめて置き換える。
///
/// x を持たせるのは、プレビューを「指している物の上」に出すため。以前は時刻だけを共有し、
/// x は毎回シークバー上の位置から引き直していた。ストリップはキーフレーム番号軸で再生位置
/// 中心なので、シークバーの時間線形な x とは無関係になり、プレビューがポインタから離れた
/// 場所に出ていた (利用者報告 2026-08-26「左端に寄ったり右にずれたり」)。
///
/// 2 つの `Option` が食い違わないよう、更新はこの関数だけが行う。
fn set_seek_preview_target(
    target_secs: &mut Option<f64>,
    anchor_x: &mut Option<f32>,
    value: Option<(f64, f32)>,
) {
    *target_secs = value.map(|(secs, _)| secs);
    *anchor_x = value.map(|(_, x)| x);
}

fn native_seek_strip_preview_hover(
    strip: &NativeOverlaySeekStrip,
    strip_rect: egui::Rect,
    pointer: egui::Pos2,
    pointer_claimed: bool,
) -> NativeSeekStripPreviewHover {
    if !pointer_claimed {
        return NativeSeekStripPreviewHover::Outside;
    }
    if strip_rect.contains(pointer)
        && native_seek_strip_lock_button_rect(strip_rect).contains(pointer)
    {
        return NativeSeekStripPreviewHover::Suppress;
    }

    let marker_x = strip_rect.center().x;
    let target = match (&strip.center, &strip.axis) {
        (
            crate::video::seek_strip::SeekStripCenter::Thumbnails { center_index },
            NativeOverlaySeekStripAxis::Ready(axis),
        ) => {
            // `cell_index_at_pointer` owns the D16 body hit convention. Once it confirms that the
            // pointer is over material, preserve the fractional position with the existing
            // continuous helper and let StripAxis own the time interpolation.
            if crate::video::seek_strip::cell_index_at_pointer(
                *center_index,
                pointer.x,
                marker_x,
                SEEK_STRIP_CELL_WIDTH,
                strip.cell_count,
            )
            .is_none()
            {
                None
            } else {
                crate::video::seek_strip::center_index_at_pointer(
                    *center_index,
                    pointer.x,
                    marker_x,
                    SEEK_STRIP_CELL_WIDTH,
                )
                .and_then(|index| axis.time_for_center_index(index))
            }
        }
        (crate::video::seek_strip::SeekStripCenter::Thumbnails { .. }, _) => None,
        (crate::video::seek_strip::SeekStripCenter::Waveform { center_time_secs }, _) => {
            crate::video::seek_strip::waveform_time_at_pointer(
                *center_time_secs,
                pointer.x,
                marker_x,
                strip_rect.width(),
                strip.waveform_span_secs,
            )
        }
    };
    match target.filter(|target| target.is_finite()) {
        Some(target) => NativeSeekStripPreviewHover::Target(target),
        None => NativeSeekStripPreviewHover::Suppress,
    }
}

fn draw_native_seek_strip_lock_button(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    lock_rect: egui::Rect,
    strip_locked: bool,
    commands: &mut Vec<NativeOverlayCommand>,
) {
    let response = ui
        .interact(
            lock_rect,
            egui::Id::new("native_video_seek_strip_lock"),
            egui::Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    // 下部 HUD のボタンは暗い HUD 帯の上に乗るので `draw_overlay_button_bg` の
    // 「非 hover は透明」で読める。ストリップのボタンはサムネイルのセルの上に乗るため、
    // 透明のままだと明るい絵に埋もれる。常に暗い下敷きと薄い縁を敷いてから、
    // hover / active の色を同じ helper で重ねる。
    painter.rect_filled(lock_rect, 4.0, egui::Color32::from_black_alpha(170));
    draw_overlay_button_bg(painter, lock_rect, response.hovered(), strip_locked);
    painter.rect_stroke(
        lock_rect,
        4.0,
        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(46)),
        egui::StrokeKind::Inside,
    );
    crate::ui_fullscreen::draw_icons::draw_seek_lock_icon(
        painter,
        lock_rect.center(),
        lock_rect.width().min(lock_rect.height()) * 0.28,
        strip_locked,
    );
    let response = response.hover_tip_dark(if strip_locked {
        "ストリップ固定を解除"
    } else {
        "ストリップを固定表示"
    });
    if response.clicked() {
        commands.push(NativeOverlayCommand::ToggleSeekStripLock);
    }
}

fn immediate_native_wheel_command(
    delta: i16,
    ctrl: bool,
    tile_overlay_visible: bool,
    over_seek_strip: bool,
    over_scroll_panel: bool,
    modal_dialog_visible: bool,
) -> Option<NativeOverlayCommand> {
    if ctrl && tile_overlay_visible {
        Some(NativeOverlayCommand::TileColumnsDelta {
            delta: if delta > 0 { -1 } else { 1 },
        })
    } else if !ctrl && !over_seek_strip && !over_scroll_panel && !modal_dialog_visible {
        Some(NativeOverlayCommand::NavigateItem {
            delta: if delta < 0 { 1 } else { -1 },
            via_wheel: true,
        })
    } else {
        None
    }
}

fn consume_seek_strip_wheel(
    ctx: &egui::Context,
    pointer_inside: bool,
) -> Vec<crate::video::seek_strip::SeekStripRangeStep> {
    if !pointer_inside {
        return Vec::new();
    }
    let steps = ctx.input(|input| {
        input
            .events
            .iter()
            .filter_map(|event| {
                let egui::Event::MouseWheel { delta, .. } = event else {
                    return None;
                };
                let dominant_delta = if delta.y.abs() >= delta.x.abs() {
                    delta.y
                } else {
                    delta.x
                };
                crate::video::seek_strip::SeekStripRangeStep::from_wheel_delta(dominant_delta)
            })
            .collect::<Vec<_>>()
    });
    if steps.is_empty() {
        return steps;
    }
    ctx.input_mut(|input| {
        input.raw_scroll_delta = egui::Vec2::ZERO;
        input.smooth_scroll_delta = egui::Vec2::ZERO;
        input
            .events
            .retain(|event| !matches!(event, egui::Event::MouseWheel { .. }));
    });
    steps
}

#[allow(clippy::too_many_arguments)]
fn draw_native_seek_strip(
    ctx: &egui::Context,
    overlay_size: egui::Vec2,
    cursor_hover_pos: Option<egui::Pos2>,
    wheel_enabled: bool,
    strip: &NativeOverlaySeekStrip,
    strip_locked: bool,
    texture_ids: &HashMap<usize, egui::TextureId>,
    wave_texture_id: Option<egui::TextureId>,
    drag_origin: &mut Option<crate::video::seek_strip::SeekStripDragOrigin>,
    last_window_request: &mut Option<(u8, u64, u64, usize, usize, usize)>,
    commands: &mut Vec<NativeOverlayCommand>,
) -> NativeSeekStripPreviewHover {
    let strip_rect = native_seek_strip_rect(overlay_size.x, overlay_size.y);
    let visible_count = ((strip_rect.width() / SEEK_STRIP_CELL_WIDTH).ceil() as usize)
        .saturating_add(2)
        .max(1);
    let pixels_per_point = ctx.pixels_per_point().clamp(1.0, 4.0);
    let pixel_width = (strip_rect.width() * pixels_per_point).round().max(1.0) as usize;
    let pixel_height = ((SEEK_STRIP_HEIGHT - 10.0) * pixels_per_point)
        .round()
        .max(1.0) as usize;
    let (mode_key, center_bits, waveform_span_bits) = match strip.center {
        crate::video::seek_strip::SeekStripCenter::Thumbnails { center_index } => {
            (0, center_index.to_bits(), 0)
        }
        crate::video::seek_strip::SeekStripCenter::Waveform { center_time_secs } => (
            1,
            center_time_secs.to_bits(),
            strip.waveform_span_secs.to_bits(),
        ),
    };
    let request_key = (
        mode_key,
        center_bits,
        waveform_span_bits,
        visible_count,
        pixel_width,
        pixel_height,
    );
    if *last_window_request != Some(request_key) {
        *last_window_request = Some(request_key);
        commands.push(NativeOverlayCommand::RequestSeekStripWindow {
            center: strip.center,
            visible_count,
            pixel_width,
            pixel_height,
        });
    }

    egui::Area::new(egui::Id::new("native_video_seek_strip"))
        .order(egui::Order::Foreground)
        .fixed_pos(strip_rect.min)
        .show(ctx, |ui| {
            ui.set_min_size(strip_rect.size());
            let local_rect = ui.min_rect();
            let painter = ui.painter().clone();
            painter.rect_filled(
                local_rect,
                0.0,
                egui::Color32::from_rgba_premultiplied(0, 0, 0, 218),
            );

            let marker_x = local_rect.center().x;
            match strip.center {
                crate::video::seek_strip::SeekStripCenter::Thumbnails { center_index } => {
                    if let Some(notice) = strip.thumbnail_notice {
                        painter.text(
                            egui::pos2((marker_x + local_rect.max.x) * 0.5, local_rect.center().y),
                            egui::Align2::CENTER_CENTER,
                            notice.label(),
                            crate::ui_fonts::hud_text_font(14.0),
                            egui::Color32::from_gray(210),
                        );
                    } else {
                        let half_cells = local_rect.width() / (2.0 * SEEK_STRIP_CELL_WIDTH);
                        // D16: cell i spans the continuous index interval [i, i + 1), so its
                        // left edge—not its center—is aligned with keyframe i.
                        let first_index = (center_index - f64::from(half_cells)).floor() as isize;
                        let last_index =
                            ((center_index + f64::from(half_cells)).ceil() - 1.0) as isize;
                        for raw_index in first_index..=last_index {
                            let Some(cell_left_x) = crate::video::seek_strip::x_for_center_index(
                                raw_index as f64,
                                center_index,
                                marker_x,
                                SEEK_STRIP_CELL_WIDTH,
                            ) else {
                                continue;
                            };
                            let cell_rect = egui::Rect::from_center_size(
                                egui::pos2(
                                    cell_left_x + SEEK_STRIP_CELL_WIDTH * 0.5,
                                    local_rect.center().y,
                                ),
                                egui::vec2(SEEK_STRIP_CELL_WIDTH - 4.0, SEEK_STRIP_HEIGHT - 10.0),
                            );
                            // 動画の範囲外には枠も背景も描かない。空のセルを枠付きで置くと、
                            // 「まだ出ていないサムネイル」と区別が付かず、黒い絵があるように見える
                            // (実機フィードバック 2026-08-24)。軸の内側で pending のセルだけ枠を出す。
                            if raw_index < 0 || (raw_index as usize) >= strip.cell_count {
                                continue;
                            }
                            let index = raw_index as usize;
                            painter.rect_filled(cell_rect, 3.0, egui::Color32::from_gray(24));
                            match strip
                                .cells
                                .iter()
                                .find(|cell| cell.index == index)
                                .map(|cell| &cell.content)
                            {
                                Some(NativeOverlaySeekStripCellContent::Ready(thumbnail)) => {
                                    if let Some(texture_id) = texture_ids.get(&index).copied() {
                                        let fitted = fit_rect_in_rect(
                                            egui::vec2(
                                                thumbnail.width as f32,
                                                thumbnail.height as f32,
                                            ),
                                            cell_rect.shrink(2.0),
                                        );
                                        painter.image(
                                            texture_id,
                                            fitted,
                                            egui::Rect::from_min_max(
                                                egui::Pos2::ZERO,
                                                egui::pos2(1.0, 1.0),
                                            ),
                                            egui::Color32::WHITE,
                                        );
                                    }
                                }
                                Some(NativeOverlaySeekStripCellContent::Failed { reason }) => {
                                    painter.text(
                                        cell_rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        reason,
                                        crate::ui_fonts::hud_text_font(13.0),
                                        egui::Color32::from_gray(190),
                                    );
                                }
                                Some(NativeOverlaySeekStripCellContent::Pending) | None => {}
                            }
                            painter.rect_stroke(
                                cell_rect,
                                3.0,
                                egui::Stroke::new(1.0, egui::Color32::from_gray(88)),
                                egui::StrokeKind::Inside,
                            );
                        }
                    }
                }
                crate::video::seek_strip::SeekStripCenter::Waveform { center_time_secs } => {
                    let wave_rect = local_rect.shrink2(egui::vec2(2.0, 5.0));
                    painter.rect_filled(wave_rect, 3.0, egui::Color32::BLACK);
                    if let (Some(texture_id), Some(wave_image)) =
                        (wave_texture_id, strip.wave_image.as_ref())
                    {
                        if let Some(slice) = crate::video::seek_strip_wave::waveform_texture_slice(
                            center_time_secs,
                            strip.waveform_span_secs,
                            wave_image.window_start_secs,
                            wave_image.window_end_secs,
                        ) {
                            let destination = egui::Rect::from_min_max(
                                egui::pos2(
                                    wave_rect.min.x + wave_rect.width() * slice.destination_start,
                                    wave_rect.min.y,
                                ),
                                egui::pos2(
                                    wave_rect.min.x + wave_rect.width() * slice.destination_end,
                                    wave_rect.max.y,
                                ),
                            );
                            let texture_rect = egui::Rect::from_min_max(
                                egui::pos2(slice.texture_start, 0.0),
                                egui::pos2(slice.texture_end, 1.0),
                            );
                            painter.image(
                                texture_id,
                                destination,
                                texture_rect,
                                egui::Color32::WHITE,
                            );
                        }
                    } else if let Some(notice) = strip.wave_notice {
                        painter.text(
                            egui::pos2((marker_x + wave_rect.max.x) * 0.5, wave_rect.center().y),
                            egui::Align2::CENTER_CENTER,
                            notice.label(),
                            crate::ui_fonts::hud_text_font(14.0),
                            egui::Color32::from_gray(210),
                        );
                    }
                    painter.rect_stroke(
                        wave_rect,
                        3.0,
                        egui::Stroke::new(1.0, egui::Color32::from_gray(88)),
                        egui::StrokeKind::Inside,
                    );
                }
            }
            painter.line_segment(
                [
                    egui::pos2(marker_x, local_rect.min.y + 3.0),
                    egui::pos2(marker_x, local_rect.max.y - 3.0),
                ],
                egui::Stroke::new(2.5, egui::Color32::from_rgb(255, 92, 92)),
            );

            let range_name = match strip.center.mode() {
                crate::settings::VideoSeekStripMode::Thumbnails => "画像間隔",
                crate::settings::VideoSeekStripMode::Waveform => "表示範囲",
            };
            let range_value = crate::video::seek_strip::format_seek_strip_range_value(
                strip.center.mode(),
                strip.range_value_secs,
            );
            let range_text = format!("{range_name} {range_value}");
            let range_pos = native_seek_strip_range_text_pos(local_rect);
            let range_font = crate::ui_fonts::hud_text_font(11.0);
            painter.text(
                range_pos + egui::vec2(1.0, 1.0),
                egui::Align2::RIGHT_TOP,
                &range_text,
                range_font.clone(),
                egui::Color32::from_black_alpha(220),
            );
            painter.text(
                range_pos,
                egui::Align2::RIGHT_TOP,
                range_text,
                range_font,
                egui::Color32::from_gray(232),
            );

            let lock_rect = native_seek_strip_lock_button_rect(local_rect);
            let response = ui.interact(
                local_rect,
                egui::Id::new("native_video_seek_strip_drag"),
                egui::Sense::click_and_drag(),
            );
            // 鍵はストリップ全面の `click_and_drag` と同じ矩形の中にある。egui は重なった
            // 領域では **後から** 登録した widget にポインタを渡すので、鍵は body の
            // `interact` より後に登録する。先に登録すると body がポインタを取り、鍵は
            // 一度も `clicked()` にならない。body 側は下の `seek_strip_body_accepts_pointer`
            // で鍵の矩形を除外するので、どちらか一方だけが反応する。
            draw_native_seek_strip_lock_button(ui, &painter, lock_rect, strip_locked, commands);
            // `Response` の hover と egui の `pointer_hover_pos` は、HUD / presenter
            // HWND 間の input handoff 中に一時的に失われ得る。HUD 可視性と同じ
            // presenter-owned native hover snapshot をストリップの矩形へ当て、可視性と
            // cursor ownership の位置正本を揃える。ドラッグ中は矩形外でも維持する。
            let pointer_inside =
                cursor_hover_pos.is_some_and(|pointer| strip_rect.contains(pointer));
            let pointer_over_lock =
                cursor_hover_pos.is_some_and(|pointer| lock_rect.contains(pointer));
            for step in consume_seek_strip_wheel(ui.ctx(), pointer_inside && wheel_enabled) {
                commands.push(NativeOverlayCommand::StepSeekStripRange { step });
            }
            if (pointer_inside && !pointer_over_lock)
                || (response.dragged() && drag_origin.is_some())
            {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            } else if pointer_over_lock {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if response.drag_started()
                && let Some(pointer) = response.interact_pointer_pos()
                && seek_strip_body_accepts_pointer(lock_rect, pointer)
            {
                *drag_origin = Some(crate::video::seek_strip::SeekStripDragOrigin::new(
                    strip.center,
                    pointer,
                ));
            }
            if response.dragged()
                && let Some(pointer) = response.interact_pointer_pos()
                && let Some(origin) = *drag_origin
            {
                if crate::video::seek_strip::strip_drag_closes_downward(
                    origin.pointer,
                    pointer,
                    strip_rect.max.y,
                ) {
                    *drag_origin = None;
                    commands.push(NativeOverlayCommand::CloseSeekStrip {
                        cause: crate::video::seek_strip::SeekStripCloseCause::DownwardDrag,
                    });
                } else {
                    let center = crate::video::seek_strip::seek_strip_center_at_drag_pointer(
                        origin,
                        pointer,
                        SEEK_STRIP_CELL_WIDTH,
                        local_rect.width(),
                        strip.waveform_span_secs,
                    );
                    if let Some(center) = center {
                        commands.push(NativeOverlayCommand::MoveSeekStrip { center });
                    }
                }
            }
            if response.drag_stopped()
                && let Some(origin) = drag_origin.take()
                && let Some(pointer) = response.interact_pointer_pos()
            {
                let center = crate::video::seek_strip::seek_strip_center_at_drag_pointer(
                    origin,
                    pointer,
                    SEEK_STRIP_CELL_WIDTH,
                    local_rect.width(),
                    strip.waveform_span_secs,
                );
                if let Some(center) = center {
                    commands.push(NativeOverlayCommand::CommitSeekStrip { center });
                }
            } else if response.clicked()
                && let Some(pointer) = response.interact_pointer_pos()
                && seek_strip_body_accepts_pointer(lock_rect, pointer)
            {
                let center = match strip.center {
                    crate::video::seek_strip::SeekStripCenter::Thumbnails { center_index } => {
                        crate::video::seek_strip::cell_index_at_pointer(
                            center_index,
                            pointer.x,
                            marker_x,
                            SEEK_STRIP_CELL_WIDTH,
                            strip.cell_count,
                        )
                        .map(|cell_index| {
                            crate::video::seek_strip::SeekStripCenter::Thumbnails {
                                center_index: cell_index as f64,
                            }
                        })
                    }
                    crate::video::seek_strip::SeekStripCenter::Waveform { center_time_secs } => {
                        crate::video::seek_strip::waveform_time_at_pointer(
                            center_time_secs,
                            pointer.x,
                            marker_x,
                            local_rect.width(),
                            strip.waveform_span_secs,
                        )
                        .map(|center_time_secs| {
                            crate::video::seek_strip::SeekStripCenter::Waveform { center_time_secs }
                        })
                    }
                };
                if let Some(center) = center {
                    commands.push(NativeOverlayCommand::CommitSeekStrip { center });
                }
            }
            let drag_claims_pointer = response.dragged() && drag_origin.is_some();
            let preview_pointer = if drag_claims_pointer {
                response.interact_pointer_pos().or(cursor_hover_pos)
            } else {
                cursor_hover_pos
            };
            preview_pointer.map_or(NativeSeekStripPreviewHover::Outside, |pointer| {
                native_seek_strip_preview_hover(
                    strip,
                    local_rect,
                    pointer,
                    pointer_inside || drag_claims_pointer,
                )
            })
        })
        .inner
}

fn draw_native_seek_strip_cycle_button(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    button_rect: egui::Rect,
    state: crate::settings::VideoSeekStripState,
    unavailable_tooltip: Option<&'static str>,
    commands: &mut Vec<NativeOverlayCommand>,
) {
    let enabled = unavailable_tooltip.is_none();
    let response = ui.interact(
        button_rect,
        egui::Id::new("native_video_seek_strip_selector"),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    draw_overlay_button_bg(
        painter,
        button_rect,
        response.hovered() && enabled,
        enabled && state != crate::settings::VideoSeekStripState::None,
    );
    draw_seek_strip_film_icon(
        painter,
        button_rect,
        if !enabled {
            egui::Color32::from_gray(105)
        } else if state == crate::settings::VideoSeekStripState::None {
            egui::Color32::from_gray(210)
        } else {
            egui::Color32::from_rgb(150, 220, 255)
        },
    );
    if enabled && state == crate::settings::VideoSeekStripState::Waveform {
        draw_seek_strip_waveform_note(painter, button_rect, egui::Color32::from_rgb(255, 220, 82));
    }
    let response = response.hover_tip_dark(if let Some(reason) = unavailable_tooltip {
        reason
    } else {
        match state {
            crate::settings::VideoSeekStripState::None => {
                "シークストリップ: オフ (クリックでサムネイル)"
            }
            crate::settings::VideoSeekStripState::Thumbnails => {
                "シークストリップ: サムネイル (クリックで音声波形)"
            }
            crate::settings::VideoSeekStripState::Waveform => {
                "シークストリップ: 音声波形 (クリックでオフ)"
            }
        }
    });
    if enabled && response.clicked() {
        commands.push(NativeOverlayCommand::SetSeekStripState {
            state: state.cycle(),
        });
    }
}

fn seek_status_visible_for_times(
    is_seeking: bool,
    started_at: Option<Instant>,
    visible_since: Option<Instant>,
    now: Instant,
) -> bool {
    let active_after_delay = is_seeking
        && started_at.is_some_and(|started| now.duration_since(started) >= SEEK_STATUS_DELAY);
    let held_after_completion =
        visible_since.is_some_and(|shown| now.duration_since(shown) < SEEK_STATUS_MIN_VISIBLE);
    active_after_delay || held_after_completion
}

fn should_claim_text_input_focus(
    text_input_active: bool,
    target_hwnd: u64,
    thread_focus_hwnd: u64,
    foreground_is_current_process: bool,
) -> bool {
    text_input_active
        && target_hwnd != 0
        && thread_focus_hwnd != target_hwnd
        && foreground_is_current_process
}

pub(crate) fn format_video_volume_db_compact(volume: f64) -> String {
    let db = crate::settings::video_volume_linear_to_db(volume);
    if db <= crate::settings::VIDEO_VOLUME_MUTE_DB + 0.05 {
        "-∞dB".to_string()
    } else if db.abs() < 0.05 {
        "0.0dB".to_string()
    } else {
        format!("{db:+.1}dB")
    }
}

pub struct NativeRenderConfig {
    pub(crate) targets: NativeRenderTargets,
    pub width: u32,
    pub height: u32,
    pub(crate) os_pixels_per_point: f32,
    pub(crate) initial_observation: NativeWindowObservation,
    pub test_overlay: bool,
    pub egui_overlay: bool,
    pub cursor_hide_delay_secs: f32,
    /// OS DPI に追加で掛けるアプリ内 UI 表示倍率。
    pub ui_scale: f32,
    /// main UI と同じ標準 / 強めの文字コントラスト。
    pub text_contrast: crate::settings::TextContrast,
    /// main UI と同じフォント指定と縦位置補正。
    pub ui_font: crate::settings::UiFontSettings,
    /// Startup-selected video scaling owner.
    pub scale_filter: VideoScaleFilter,
    pub downscale_smoothing_percent: u32,
    pub anime4k_variant: Option<crate::video::anime4k_policy::VideoAnime4kVariant>,
    pub anime4k_budget: crate::video::anime4k_policy::VideoAnime4kBudgetPreset,
    pub anime4k_status: NativeVideoAnime4kStatus,
    pub(crate) health: Arc<crate::video::native_window_health::NativeWindowHealth>,
    pub(crate) window_epoch: u64,
}

enum NativeOverlayCompositionTarget {
    Presenter,
    Hud {
        _target: IDCompositionTarget,
        _root_visual: IDCompositionVisual,
    },
}

pub struct NativeRenderCore {
    health: Arc<crate::video::native_window_health::NativeWindowHealth>,
    window_epoch: u64,
    swap_chain: IDXGISwapChain1,
    waitable: HANDLE,
    d3d_device1: ID3D11Device1,
    d3d_device5: ID3D11Device5,
    d3d_context: windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
    d3d_context1: ID3D11DeviceContext1,
    d3d_context4: ID3D11DeviceContext4,
    _dcomp_device: IDCompositionDevice,
    _dcomp_target: IDCompositionTarget,
    _root_visual: IDCompositionVisual,
    _background: NativeBlackBackground,
    _video_visual: IDCompositionVisual,
    backbuffer: Option<ID3D11Texture2D>,
    test_overlay: Option<NativeTestOverlay>,
    egui_overlay: Option<NativeEguiOverlay>,
    window_observation: NativeWindowObservation,
    _overlay_composition_target: NativeOverlayCompositionTarget,
    /// 実機修正 (2026-05-12 P1 #3): 直近 LBUTTON down が検出された時刻。
    /// external_drag 判定に 100ms の delay を入れることで「short click」を
    /// drag と誤検出して top bar を hide させるバグを防ぐ。
    lbutton_down_since: Option<Instant>,
    fence_cache: Option<(u64, isize, ID3D11Fence)>,
    /// presenter 自前の D3D11 fence。`copy_frame_into_backbuffer` のフレームコピー後に
    /// `Signal` して値を進める。`run_native_video_output` 側の `present_retire` は
    /// `copy_fence_completed_value()` を見て、コピーが GPU 上で完了したフレームだけを
    /// 解放する (= 共有出力 slot を「presenter のコピー完了後」に返す保証)。
    /// fence 作成に失敗した環境では `None` で、`present_retire` は時間ベースの depth
    /// キャップにフォールバックする。
    copy_fence: Option<ID3D11Fence>,
    /// `copy_fence` に次に `Signal` する値 (1, 2, 3, ... と単調増加)。
    copy_fence_value: u64,
    /// 開いた共有出力テクスチャのキャッシュ。キーは `(NT shared handle 値, shared_texture_gen)`。
    /// `gen` を含める理由は `open_shared_texture` のドキュメントコメント参照
    /// (= handle 値再利用による前動画フレーム混入の防止)。
    shared_texture_cache: Vec<((u64, u64), ID3D11Texture2D)>,
    cpu_upload_scratch: Vec<u8>,
    video_grade: crate::creative_lut::VideoGradeSnapshot,
    grade_pipeline: VideoGradePipeline,
    panorama_pipeline: Option<VideoPanoramaPipeline>,
    panorama_pipeline_error: Option<String>,
    panorama_pose: Option<(PanoPose, PanoUvTransform)>,
    resample_pipeline: Option<VideoResamplePipeline>,
    resample_pipeline_error: Option<String>,
    selected_scale_filter: VideoScaleFilter,
    downscale_smoothing_percent: u32,
    selected_anime4k_variant: Option<crate::video::anime4k_policy::VideoAnime4kVariant>,
    anime4k_budget: crate::video::anime4k_policy::VideoAnime4kBudgetPreset,
    anime4k_status: NativeVideoAnime4kStatus,
    surface_content: VideoSurfaceContent,
    resize_settle: VideoResizeSettleState,
    prepared_video_surface: Option<PreparedVideoSurface>,
    prepared_video_scale_settings: Option<PreparedVideoScaleSettings>,
    active_scale_fallback: Option<ActiveVideoScaleFallback>,
    scale_fallback_count: u64,
    surface_swap_count: u64,
    pixel_probe_enabled: bool,
    pixel_probe_strict: bool,
    last_pixel_probe: Option<Instant>,
    video_compact: bool,
    video_top_bar_locked: bool,
    video_bottom_lock: VideoBottomLock,
    fullscreen_fixed_bar_gap_px: u32,
    /// Sample aspect ratio (= pixel aspect ratio)。1/1 = 正方ピクセル (= 従来挙動)。
    /// アナモフィック動画 (NTSC DVD 等で SAR=97/80 など) で `update_video_visual_transform`
    /// の M11/M22 を anisotropic にして表示比を補正する。decoder の VideoInfo 経由で
    /// `set_video_geometry(num, den, orientation)` で設定される。
    sar_num: u32,
    sar_den: u32,
    orientation: VideoOrientation,
    width: u32,
    height: u32,
    surface_width: u32,
    surface_height: u32,
    /// 動画フレームの解像度が変わったとき、新しい swap chain を作るための DXGI factory。
    /// `new()` 生成時のものを保持する。
    factory: IDXGIFactory2,
    /// 解像度変更で差し替えた旧 video swap chain を遅延破棄するためのキュー。
    /// `SetContent` で新 swap chain に切り替えた後も DComp / DWM 側がしばらく旧 content を
    /// 参照しうるため、即 drop せず数世代分保持する (Codex 助言)。
    retired_video_surfaces: VecDeque<RetiredVideoSurface>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NativeRenderCoreDetachTimings {
    pub(crate) egui_overlay_ms: f64,
    pub(crate) rest_ms: f64,
}

/// 解像度変更で差し替えた旧 video swap chain。`retired_video_surfaces` で遅延保持し、
/// キューから押し出されたタイミングで Drop される (= swap chain + waitable を解放)。
struct RetiredVideoSurface {
    _swap_chain: IDXGISwapChain1,
    waitable: HANDLE,
    _backbuffer: Option<ID3D11Texture2D>,
}

struct PreparedVideoSurface {
    width: u32,
    height: u32,
    content: VideoSurfaceContent,
    swap_chain: Option<IDXGISwapChain1>,
    waitable: HANDLE,
    backbuffer: Option<ID3D11Texture2D>,
}

impl PreparedVideoSurface {
    fn matches(&self, width: u32, height: u32, content: VideoSurfaceContent) -> bool {
        self.width == width && self.height == height && self.content == content
    }
}

impl Drop for PreparedVideoSurface {
    fn drop(&mut self) {
        if !self.waitable.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.waitable);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct VideoScalePreparationSignature {
    source_width: u32,
    source_height: u32,
    sar_num: u32,
    sar_den: u32,
    orientation: VideoOrientation,
    target_width: u32,
    target_height: u32,
    target_content: VideoSurfaceContent,
    panorama_active: bool,
    grade_active: bool,
    filter: VideoScaleFilter,
    smoothing_percent: u32,
    anime4k_variant: Option<crate::video::anime4k_policy::VideoAnime4kVariant>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PreparedVideoScaleSettings {
    identity: crate::video::VideoScaleRequestIdentity,
    signature: VideoScalePreparationSignature,
}

fn video_scale_signature_changed_fields(
    prepared: VideoScalePreparationSignature,
    current: VideoScalePreparationSignature,
) -> Vec<&'static str> {
    let mut fields = Vec::new();
    macro_rules! changed {
        ($field:ident) => {
            if prepared.$field != current.$field {
                fields.push(stringify!($field));
            }
        };
    }
    changed!(source_width);
    changed!(source_height);
    changed!(sar_num);
    changed!(sar_den);
    changed!(orientation);
    changed!(target_width);
    changed!(target_height);
    changed!(target_content);
    changed!(panorama_active);
    changed!(grade_active);
    changed!(filter);
    changed!(smoothing_percent);
    changed!(anime4k_variant);
    fields
}

fn validate_prepared_video_scale_settings(
    prepared: Option<PreparedVideoScaleSettings>,
    identity: crate::video::VideoScaleRequestIdentity,
    signature: VideoScalePreparationSignature,
) -> Result<(), crate::video::VideoScaleApplyRejectReason> {
    let prepared = prepared.ok_or(crate::video::VideoScaleApplyRejectReason::NoPrepared)?;
    if prepared.identity.source_epoch != identity.source_epoch {
        return Err(crate::video::VideoScaleApplyRejectReason::SourceChanged {
            prepared_source_epoch: prepared.identity.source_epoch,
            current_source_epoch: identity.source_epoch,
        });
    }
    if prepared.identity.revision != identity.revision {
        return Err(crate::video::VideoScaleApplyRejectReason::RequestMismatch {
            prepared_revision: prepared.identity.revision,
            current_revision: identity.revision,
        });
    }
    let fields = video_scale_signature_changed_fields(prepared.signature, signature);
    if !fields.is_empty() {
        return Err(crate::video::VideoScaleApplyRejectReason::SignatureChanged { fields });
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) enum VideoScalePreparedPresentError {
    Rejected(crate::video::VideoScaleApplyRejectReason),
    Present(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VideoSurfaceContent {
    LegacySource,
    DisplayResolved,
    PanoramaResolved,
}

impl VideoSurfaceContent {
    fn resolved(panorama_active: bool) -> Self {
        if panorama_active {
            Self::PanoramaResolved
        } else {
            Self::DisplayResolved
        }
    }

    fn is_resolved(self) -> bool {
        !matches!(self, Self::LegacySource)
    }
}

#[derive(Clone, Copy, Debug)]
enum VideoResizeSettleState {
    Settled,
    Waiting { last_geometry_change: Instant },
}

impl VideoResizeSettleState {
    fn is_resizing(self, now: Instant) -> bool {
        matches!(
            self,
            Self::Waiting {
                last_geometry_change
            } if now.saturating_duration_since(last_geometry_change) < VIDEO_RESIZE_SETTLE_INTERVAL
        )
    }

    fn refresh_due(self, now: Instant) -> bool {
        matches!(
            self,
            Self::Waiting {
                last_geometry_change
            } if now.saturating_duration_since(last_geometry_change) >= VIDEO_RESIZE_SETTLE_INTERVAL
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DisplaySurfaceRequestKey {
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
    sar_num: u32,
    sar_den: u32,
    orientation: VideoOrientation,
    panorama_active: bool,
    grade_active: bool,
    filter: VideoScaleFilter,
    smoothing_percent: u32,
    anime4k_variant: Option<crate::video::anime4k_policy::VideoAnime4kVariant>,
}

#[derive(Clone, Debug)]
struct ActiveVideoScaleFallback {
    key: DisplaySurfaceRequestKey,
    reason: VideoScaleFallbackReason,
}

enum VideoSurfaceSwapError {
    DisplayCreation(VideoScaleFallbackReason),
    Other(String),
}

impl From<String> for VideoSurfaceSwapError {
    fn from(error: String) -> Self {
        Self::Other(error)
    }
}

impl VideoSurfaceSwapError {
    fn into_string(self) -> String {
        match self {
            Self::DisplayCreation(reason) => {
                format!("display surface creation failed: {}", reason.detail())
            }
            Self::Other(error) => error,
        }
    }
}

impl Drop for RetiredVideoSurface {
    fn drop(&mut self) {
        if !self.waitable.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.waitable);
            }
        }
    }
}

/// 旧 video swap chain を保持する世代数。`SetContent` 切替後、DComp/DWM が旧 content を
/// 参照しなくなるまでの猶予 + presenter の非同期 GPU コピー完了の猶予を兼ねる。
///
/// 2026-05-15 (Codex 助言): 旧 3 → 1 に縮小。swap chain は `BufferCount=3` なので
/// 4K で 1 個約 95 MB。depth=3 だと active + retired 4 個で 4K 単独 ~380 MB の
/// adapter memory を占有していた。fast-swap で前動画分も加わると数 GB 級になり
/// `wgpu Out of Memory` panic に直結。depth=1 でも次フレームまで旧 surface は
/// 保持されるので「片チャンネル切替の瞬間に旧 surface が GPU 上に必要」要件を
/// 満たす (`SetContent` の Commit が DComp に届くまで 1 frame 程度のラグ)。
const RETIRED_VIDEO_SURFACE_DEPTH: usize = 1;

struct NativeBlackBackground {
    swap_chain: IDXGISwapChain1,
    _visual: IDCompositionVisual,
    backbuffer: Option<ID3D11Texture2D>,
    render_target: Option<ID3D11RenderTargetView>,
    width: u32,
    height: u32,
}

struct NativeTestOverlay {
    swap_chain: IDXGISwapChain1,
    _visual: IDCompositionVisual,
    backbuffer: Option<ID3D11Texture2D>,
    render_target: Option<ID3D11RenderTargetView>,
    width: u32,
    height: u32,
    /// true = MPO 防止用の完全透明カバー (テストパターンを描かず透明のまま present)。
    transparent: bool,
}

struct NativeEguiOverlay {
    health: Arc<crate::video::native_window_health::NativeWindowHealth>,
    window_epoch: u64,
    surface: wgpu::Surface<'static>,
    visual: IDCompositionVisual,
    dcomp_device: IDCompositionDevice,
    root_visual: IDCompositionVisual,
    /// この visual を root にぶら下げる際に「どの sibling の後ろに配置するか」。
    /// `Some(v)` なら presenter フォールバック経路で `video_visual` の後ろに挟む。
    /// `None` なら HUD HWND の DComp root に単独で配置する (CP3 P1 #3 反映)。
    after_visual: Option<IDCompositionVisual>,
    gpu_epoch: Arc<DeviceEpoch>,
    format: wgpu::TextureFormat,
    present_mode: wgpu::PresentMode,
    alpha_mode: wgpu::CompositeAlphaMode,
    renderer: egui_wgpu::Renderer,
    egui_ctx: egui::Context,
    /// Opaque DComp/DPI target lease. It carries no window lifecycle capability.
    _dcomp_target_lease: NativeRenderTarget,
    /// Pump-owned USER32 state, copied as a value snapshot.
    window_observation: NativeWindowObservation,
    /// テキスト入力ダイアログ表示中に presenter HWND へ focus を戻した時刻。
    /// HUD HWND は `WS_EX_NOACTIVATE` なので、別アプリから mIV に戻っただけでは
    /// OS focus が main/HUD 側に残り、presenter wndproc に key/IME が来ない場合がある。
    /// mIV が前面に戻っている時だけ presenter に戻すためのレート制限。
    last_text_input_focus_claim_at: Option<Instant>,
    started_at: Instant,
    pending_events: Vec<egui::Event>,
    native_touch: crate::video::native_touch::NativeTouchAdapter,
    panorama_pose: Option<PanoPose>,
    modifiers: egui::Modifiers,
    pointer_pos: Option<egui::Pos2>,
    event_count: u64,
    render_count: u64,
    dirty: bool,
    next_repaint_deadline: Option<Instant>,
    wants_pointer_input: bool,
    wants_keyboard_input: bool,
    video_position_secs: f64,
    video_duration_secs: f64,
    video_is_playing: bool,
    video_volume: f64,
    video_muted: bool,
    video_limiter_ceiling_hit_seq: u64,
    video_limiter_visible_until: Option<Instant>,
    video_playback_speed: f64,
    video_frame_step_active: bool,
    video_is_seeking: bool,
    video_seek_serial: u64,
    seek_status_started_at: Option<Instant>,
    seek_status_visible_since: Option<Instant>,
    seek_status_visible: bool,
    video_speed_popup_open: bool,
    frame_step_hold: Option<NativeFrameStepHold>,
    video_loop_enabled: bool,
    /// HUD ボタン表示用のループモード (= ユーザー設定の display_mode)。
    /// 「BM 設定 + BM 無し動画」のとき、`video_loop_enabled` は effective から導出した
    /// bool が入るが、`video_loop_mode` は表示用に Bookmark のまま維持する。
    video_loop_mode: crate::settings::VideoLoopMode,
    video_continuous_mode: crate::video::VideoContinuousMode,
    video_checked: bool,
    vst3_available: bool,
    hud_dimmed: bool,
    /// 音声のみ native シェル (music Inc 6 ②)。上バーで動画専用ボタン (タイル一覧 / Perf
    /// グラフ) を出さず、音楽ビューと同じ VST・フルスクリーン切替・閉じるだけにする。
    audio_only: bool,
    vst3_panel: Option<NativeOverlayVst3Panel>,
    first_frame_presented: bool,
    video_error: Option<String>,
    /// 動画オープン中の進捗 (phase / bytes_read / file_size)。
    /// `first_frame_presented = false` の間だけ center status HUD に出る。
    preparing_status: crate::video::avio_progress::PreparingStatus,
    toast: Option<NativeOverlayToast>,
    perf_visible: bool,
    perf_history: VecDeque<NativeOverlayPerfSample>,
    perf_latest: NativeOverlayPerfSnapshot,
    perf_last_dirty: Instant,
    perf_pause_gap_pending: bool,
    last_seek_target_secs: Option<f64>,
    last_thumbnail_request_secs: Option<f64>,
    last_thumbnail_request_at: Option<Instant>,
    hover_preview_target_secs: Option<f64>,
    /// `hover_preview_target_secs` を出した画面上の x。更新は `set_seek_preview_target` だけ。
    hover_preview_anchor_x: Option<f32>,
    hover_preview_pinned: bool,
    /// 実機修正 (2026-05-12 P1 #2): 直近 egui run で描画した preview rect (= サムネイル枠)。
    /// `compute_hud_regions` が region 計算で参照する。region を cursor x 追従で再計算すると
    /// 描画 rect (= `target_secs` 起点) とずれて「サムネ画像は固定なのに枠だけ動く」症状になる。
    /// `None` ならサムネ非表示。
    last_drawn_preview_rect: Option<egui::Rect>,
    /// 実機修正 (2026-05-12 A): 直近 egui run で描画した VST3 設定パネルの actual rect。
    /// パネルは `egui::Area::movable(true)` でドラッグ可能なので、デフォルト位置 (=
    /// `native_vst3_panel_rect`) からドラッグでずれた場合、region をその実位置に追従させる。
    /// `None` ならパネル非表示。
    last_drawn_vst3_panel_rect: Option<egui::Rect>,
    /// Off-screen clamp 復旧などで同じ VST3 panel 位置 command を複数フレーム連続発行しないための
    /// presenter-local 記憶。UI thread から新しい panel_pos snapshot が届いたらリセットする。
    last_emitted_vst3_panel_pos: Option<[f32; 2]>,
    /// 直近 egui run で描画した toast の actual rect。
    /// HUD HWND は `SetWindowRgn` で領域外が物理的に clip されるため、toast も
    /// region に含めないと他 UI の region に重なった部分だけが見える。
    last_drawn_toast_rect: Option<egui::Rect>,
    /// 直近 egui run で描画した playback speed popup の actual rect。
    /// speed ボタンは下 HUD 右側にあるため、中央固定の概算 region だと popup が
    /// SetWindowRgn 外に落ちて見えたり、click hit-test が不安定になったりする。
    /// `None` なら popup 非表示または未描画。
    last_drawn_speed_popup_rect: Option<egui::Rect>,
    /// 直近 egui run で描画したブックマーク名編集ダイアログの actual rect。
    /// 中央モーダルだが、配置は `pos.y = (H - dialog_h) * 0.5` (dialog_h はレイアウト
    /// 用の過大見積もり) で、実コンテンツ高さとの差でダイアログ中心が画面中心より
    /// 上にずれる。画面中心固定の概算 region だと上端 (=「ブックマーク名」ラベル) が
    /// SetWindowRgn でクリップされるため、実描画 rect を region に使う。
    /// `None` ならダイアログ非表示または未描画。
    last_drawn_bookmark_editor_rect: Option<egui::Rect>,
    /// 直近 egui run で描画した一括ブックマーク登録ダイアログの actual rect。
    /// ダイアログは「確認モード」のあるなしや TextEdit の表示行数で高さが変動するため、
    /// 固定見積もり (560×420) では下部ボタンが SetWindowRgn から落ちることがある
    /// (= ボタンクリックが seek bar / video HWND に抜ける、ホバー外側でカーソル形状が
    /// 戻らない、等の症状。Codex P2 #1 2026-05-24)。`None` ならダイアログ非表示または未描画。
    last_drawn_bulk_bookmark_dialog_rect: Option<egui::Rect>,
    /// 直近 egui run で描画したショートカットヘルプダイアログの actual rect。
    /// `?` で開く中央モーダルも HUD HWND region に含めないと clip / click-through する。
    last_drawn_shortcut_help_rect: Option<egui::Rect>,
    /// 直近 egui run で描画したゲームパッド X ピッカー overlay の actual rect。
    /// X ピッカーは App 側が入力を処理し、native overlay は表示専用だが、
    /// HUD HWND の region に含めないと SetWindowRgn で中央パネルがクリップされる。
    /// `None` ならピッカー非表示または未描画。
    last_drawn_ring_picker_rect: Option<egui::Rect>,
    /// 直近 egui run で描画したリングガイド overlay の actual rect。
    /// X+方向リング / 右ドラッグフリックのガイドは App 側が入力を処理し、native overlay は
    /// 表示専用。HUD HWND の region に含めるために実描画 rect を記録する。
    last_drawn_ring_guide_rect: Option<egui::Rect>,
    hover_thumbnail: Option<NativeOverlayThumbnail>,
    hover_texture: Option<egui::TextureHandle>,
    hover_texture_key: Option<(u32, u32, u64)>,
    timeline_markers: Vec<NativeOverlayTimelineMarker>,
    jump_entries: Vec<NativeOverlayJumpEntry>,
    video_adjustments: crate::creative_lut::VideoAdjustments,
    video_scale_filter: VideoScaleFilter,
    video_downscale_smoothing_percent: u32,
    video_scale_outside_note: Option<String>,
    video_anime4k_budget: crate::video::anime4k_policy::VideoAnime4kBudgetPreset,
    video_anime4k_status: NativeVideoAnime4kStatus,
    video_preset_slots: Arc<[Option<String>]>,
    creative_lut_choices: Arc<[crate::creative_lut::CreativeLutChoice]>,
    video_left_panel_tab: NativeVideoLeftPanelTab,
    video_adjustment_tab: NativeVideoAdjustmentTab,
    bookmark_title_edit: Option<NativeBookmarkTitleEdit>,
    bulk_bookmark_dialog: Option<NativeBulkBookmarkDialog>,
    shortcut_help_open: bool,
    tag_picker_open: bool,
    tag_picker_input: String,
    tag_picker_focus_request: bool,
    tag_picker_recent_tab: bool,
    tag_panel_sticky_item_key: Option<String>,
    tag_panel_sticky_tags: Vec<NativeOverlayTagDef>,
    video_metadata: Option<NativeOverlayMetadata>,
    fallback_file_name: String,
    navigation_preview: Option<NativeOverlayNavigationPreview>,
    navigation_preview_texture: Option<(u64, egui::TextureHandle)>,
    tile_overlay: Option<NativeOverlayTileOverlay>,
    seek_strip: Option<NativeOverlaySeekStrip>,
    seek_strip_material_availability: crate::video::seek_strip::SeekStripMaterialAvailability,
    ring_picker_overlay: Option<NativeOverlayRingPicker>,
    ring_guide_overlay: Option<NativeOverlayRingGuide>,
    tile_textures: HashMap<usize, (u64, egui::TextureHandle)>,
    seek_strip_textures: HashMap<usize, (u64, egui::TextureHandle)>,
    seek_strip_wave_texture: Option<(u64, egui::TextureHandle)>,
    seek_row_gesture: Option<crate::video::seek_strip::SeekRowGesture>,
    seek_strip_drag_origin: Option<crate::video::seek_strip::SeekStripDragOrigin>,
    last_seek_strip_window_request: Option<(u8, u64, u64, usize, usize, usize)>,
    jump_textures: HashMap<usize, (u64, egui::TextureHandle)>,
    /// 直前の render_once で実際に描画した上部バーの可視状態。
    /// top_bar_visible は hover ヒステリシス用に残し、region はこの snapshot を参照する。
    top_bar_drawn_visible: bool,
    /// 直前の render_once で実際に描画した下部バーの可視状態。
    /// HWND hit-test region は上下とも再判定せず、描画 snapshot を参照する。
    bottom_hud_visible: bool,
    /// 上部バーの hover ヒステリシス状態。region の入力には使わない。
    top_bar_visible: bool,
    right_panel_visible: bool,
    jump_panel_visible: bool,
    /// App settings から同期される、上部バーと動画下部の固定状態。
    top_bar_locked: bool,
    bottom_lock: VideoBottomLock,
    /// App settings から同期される、左右パネル共通の表示モード。
    side_panel_mode: FsSidePanelMode,
    /// 右情報パネルの明示 open owner。正本は App の current-file typed state。
    info_panel_open: crate::ui_helpers::MetadataPanelOpenState,
    /// 左ジャンプパネルの明示 open owner。動画ソース単位の presenter-local session。
    /// pointer と touch handle を同じ typed state で区別し、外タップ dismiss の正本にする。
    left_panel_open: crate::ui_helpers::MetadataPanelOpenState,
    /// 右パネル (情報/★/タグ) の端ホバー開閉ラッチ。画面右端 5% の細いトリガで開き、
    /// パネル矩形 + ヒステリシス余白から出るまで維持する。パネル幅ぶんの広い当たり判定が
    /// 中央のクリック (右クリックページ送り等) を食う問題への対策 (実機 FB 2026-07)。
    /// egui / 音楽ビュー側の二段ラッチと挙動を揃える。`update_side_panel_hover_latches` が
    /// フレーム先頭で更新し、`right_panel_visible()` が読む。
    right_panel_hover_latched: bool,
    /// 左ジャンプ・情報パネルの端ホバー開閉ラッチ。右と同じ二段判定。
    jump_panel_hover_latched: bool,
    /// ClickToShow の開いたパネルを Escape で閉じた入力 batch を App へ流さないための印。
    side_panel_escape_consumed: bool,
    /// 実機修正 (2026-05-12): 外部 drag (= HUD region 外で left button down 中、典型的には VST window
    /// のドラッグ) を検出するフラグ。pump が配る global left-button snapshot と
    /// egui の `pointer.any_down()` の差から判定して set する。
    /// true の間、`top_bar_visible()` / `hud_visible()` / `right_panel_visible()` 等の hover 判定を
    /// 強制 false にして、bar / panel が出ないようにする (= VST 上端帯にドラッグしても hover 表示で
    /// VST 入力が奪われないようにする)。
    external_drag_in_progress: bool,
    /// parked/dimmed HUD では egui への pointer 配送を止めるが、HUD の fade in/out は
    /// raw cursor hover に追従させる。`pointer_pos` は egui 入力、こちらは可視性と
    /// cursor ownership 判定専用。
    raw_hover_pos: Option<egui::Pos2>,
    pending_overlay_commands: Vec<NativeOverlayCommand>,
    last_volume_target: Option<f64>,
    visual_attached: bool,
    /// main egui Context の zoom_factor をミラーするアプリ内倍率。
    ui_scale: f32,
    pixels_per_point: f32,
    width: u32,
    height: u32,
    // Cursor policy is emitted to the pump-owned reducer. Activity, hidden
    // state, and input ownership do not live on the render thread.
    /// 音量ノーマライズ UI 状態 (App から `SetNormalizeOverlayState` で配信される)。
    normalize_state: crate::video::normalize_types::NormalizeOverlayState,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct NativeBarVisibilitySnapshot {
    /// tile/navigation guard と side panel 連動まで通した、上部バーの実描画状態。
    top_bar_visible: bool,
    /// 上端 hover 連動と下部自身の固定状態まで通した、下部バーの実描画状態。
    bottom_hud_visible: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct NativeFrameStepHold {
    pub(super) direction: i32,
    pub(super) last_step_at: Instant,
}

#[derive(Clone, Debug)]
pub(crate) struct NativeBookmarkTitleEdit {
    pub(crate) id: i64,
    pub(crate) title: String,
    pub(crate) request_focus: bool,
}

/// 一括ブックマーク登録ダイアログの永続 state。
/// `Some` の間ダイアログが描画される。`textarea` にユーザーがペーストした
/// チャプターテキストが入り、登録ボタンで `BulkAddBookmarks` コマンドを発行する。
#[derive(Clone, Debug)]
pub(crate) struct NativeBulkBookmarkDialog {
    pub(crate) textarea: String,
    pub(crate) request_focus: bool,
    pub(crate) confirm_clear_all: bool,
    /// Ctrl+V で読み出したテキストの first-paste 救済 (Codex C8)。`Some` のとき、
    /// 次の draw で textarea のカーソル位置に挿入する (Event::Paste が focus を
    /// 持たない TextEdit に届いて捨てられる race を回避)。挿入後 `None` に戻す。
    pub(crate) pending_paste: Option<String>,
    /// エクスポートチェックボックス: 「秒単位にする」。`true` のとき整数秒へ floor、
    /// `false` のとき小数 3 桁 (ms 精度) で出力する。ダイアログを閉じると消える
    /// (= 次回開いたときは既定値 `true` に戻る、永続化しない設計)。
    pub(crate) export_seconds_only: bool,
}

impl Default for NativeBulkBookmarkDialog {
    fn default() -> Self {
        Self {
            textarea: String::new(),
            request_focus: false,
            confirm_clear_all: false,
            pending_paste: None,
            // 外部のコメント自動リンク化は秒単位しか拾わないため、互換性の高い秒単位を既定にする。
            export_seconds_only: true,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct NativeOverlayToast {
    pub(super) text: String,
    pub(super) started_at: Instant,
    pub(super) centered: bool,
    /// このトーストを表示し続ける時間。`started_at` からこれを過ぎたら消す。
    /// `show_toast` で `linger` 指定があればその値、無ければ `centered` から導いた
    /// 既定値 (centered: 2.5s / それ以外: 1.8s) が入る。
    linger: Duration,
}

#[cfg(test)]
impl NativeOverlayToast {
    fn for_test(text: &str) -> Self {
        Self {
            text: text.to_string(),
            started_at: Instant::now(),
            centered: false,
            linger: Duration::from_secs_f32(1.8),
        }
    }
}

pub struct NativePresentOutcome {
    pub path: &'static str,
    pub shared_handle: u64,
    pub shared_cache_hit: bool,
    /// この present で参照した共有出力テクスチャの世代 ID (GPU 経路のみ、CPU 経路は 0)。
    pub shared_texture_gen: u64,
    /// この present で参照した GPU frame の fence value (GPU 経路のみ、CPU 経路は 0)。
    pub fence_value: u64,
    /// このフレームコピーの GPU 完了に対応する presenter copy fence の値。
    /// `present_retire` がこの値の到達を見てフレームを解放する。fence 未作成時は 0。
    pub copy_fence_value: u64,
    /// この present 完了後の video swap chain サーフェスサイズ。
    pub surface_width: u32,
    pub surface_height: u32,
    /// この present でフレーム実寸 / SAR が変わり transform を更新したか。
    pub geometry_changed: bool,
    /// この present で解像度変更により video swap chain を原子的に差し替えたか。
    pub surface_swapped: bool,
    /// 差し替え時の `WaitForCommitCompletion` 待ち時間 (ms)。差し替えが無ければ 0。
    pub commit_sync_ms: f64,
    pub wait_ms: f64,
    pub wait_timed_out: bool,
    pub fence_wait_ms: f64,
    pub open_shared_ms: f64,
    pub keyed_mutex_ms: f64,
    pub keyed_mutex_cast_ms: f64,
    pub keyed_mutex_acquire_ms: f64,
    pub copy_call_ms: f64,
    pub copy_ms: f64,
    pub present_waitable_ms: f64,
    pub present_call_ms: f64,
    pub present_ms: f64,
}

/// `copy_frame_into_backbuffer` の戻り値。GPU / CPU いずれの経路でフレームを
/// backbuffer へコピーしたかと、その計測値をまとめる。
struct FrameCopyMetrics {
    path: &'static str,
    shared_handle: u64,
    shared_cache_hit: bool,
    shared_texture_gen: u64,
    fence_value: u64,
    fence_wait_ms: f64,
    open_shared_ms: f64,
    keyed_mutex_ms: f64,
    keyed_mutex_cast_ms: f64,
    keyed_mutex_acquire_ms: f64,
    copy_call_ms: f64,
    /// このコピーの GPU 完了に対応する presenter copy fence の値。`present_retire` は
    /// `copy_fence_completed_value() >= この値` になったフレームだけを解放する。
    /// fence 未作成時は 0 (= ゲートに使わない、depth キャップのみ)。
    copy_fence_value: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeOverlayPerfSnapshot {
    pub elapsed_secs: f64,
    pub presented: u64,
    pub gpu: u64,
    pub cpu: u64,
    pub late_drop: u64,
    pub wait_timeout: u64,
    pub actual_fps: f64,
    pub max_late_ms: f64,
    pub max_total_ms: f64,
    pub max_interval_ms: f64,
    /// **video pacing health 指標** (= video_pts − master_clock、ms 単位、符号付き)。
    /// 通常 ≈ 0。これだけでは Norm 経路バグなど audio が clock から乖離した場合の
    /// 体感ズレを検出できないので、ユーザー表示は `av_offset_ms` を主にする。
    pub av_drift_ms: f32,
    /// **ユーザー体感の音映像差** (= video_pts − audio_audible_pts、ms、符号付き)。
    /// + = 映像が音声より進んでいる、− = 映像が音声より遅れている。
    /// audio inactive (動画 only / 音声起動失敗) または seek 直後など offset 未確定時は
    /// `f32::NAN`。audio の有無は `audio_active` を見る。
    pub av_offset_ms: f32,
    /// audio stream が clock source として active か。`av_offset_ms` は seek 直後に一時
    /// NaN になるので、lead / underrun の表示可否はこの値で判定する。
    pub audio_active: bool,
    /// audio が master clock より何 ms 先行しているか (callback 直近値)。
    /// 通常 ≈ 0、Norm clear 後の big jump 直後は 5000+ ms に張り付くことがある。
    pub audio_lead_ms: f32,
    /// callback で silence を出力中か (= cpal underrun 中)。pump 復活で false に戻る。
    pub audio_underrun_active: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct NativeOverlayPerfSample {
    pub arrival: Instant,
    pub interval_ms: f32,
    pub total_ms: f32,
    pub copy_ms: f32,
    pub present_waitable_ms: f32,
    pub present_call_ms: f32,
    pub late_ms: f32,
    /// 本 sample の直前選択で表示前に捨てた frame 数。
    /// perf graph の赤縦線は frame interval の揺れではなく、この値だけを marker にする。
    pub late_drop_delta: u32,
    /// PTS delta between consecutive presented frames (= 1/native_fps の生値)。
    /// 再生速度の影響を受けない (= 30fps なら常に 33.33ms)。Y 軸 / gap 判定には
    /// `playback_speed` で割って実 frame interval に正規化したものを使う。
    pub source_delta_ms: f32,
    /// この sample が記録された時点の再生速度倍率 (= 0.25..=4.0)。
    /// 0.5x なら実 frame interval は `source_delta_ms / 0.5 = 2 * source_delta_ms`。
    pub playback_speed: f32,
    /// 本 sample 取得時点の video pacing drift (signed ms、video_pts − master_clock)。
    pub av_drift_ms: f32,
    /// 本 sample 取得時点の **体感音映像差** (signed ms、video_pts − audio_audible_pts)。
    /// audio inactive または seek 直後など offset 未確定時は `f32::NAN`。
    /// グラフのサブトラック描画はこちらを優先。
    pub av_offset_ms: f32,
    /// 本 sample 取得時点で audio stream が active だったか。
    pub audio_active: bool,
    /// 本 sample 取得時点で audio が master clock から先行している量 (ms)。
    /// 通常 ≈ 0、Norm clear 直後は >>0 に張り付く。
    pub audio_lead_ms: f32,
    /// 本 sample 取得時点で audio underrun 中だったか (橙背景帯の描画用)。
    pub audio_underrun_active: bool,
}

#[derive(Clone, Debug)]
pub struct NativeOverlayThumbnail {
    pub target_secs: f64,
    pub match_tolerance_secs: f64,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeOverlayTimelineMarkerKind {
    Pin,
    Bookmark,
    Chapter,
}

#[derive(Clone, Copy, Debug)]
pub struct NativeOverlayTimelineMarker {
    pub pts_secs: f64,
    pub kind: NativeOverlayTimelineMarkerKind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeOverlayTagDef {
    pub name: String,
    pub tag_key: String,
    pub count: usize,
    pub pinned: bool,
    pub last_applied_at: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeOverlayMetadata {
    pub item_key: String,
    pub file_name: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub original_url: Option<String>,
    pub description: Option<String>,
    pub probe_info_available: bool,
    /// ★ レーティング (0..=5)。右パネル先頭の★行に表示 (画像/動画/音声で統一、Inc 5 FB)。
    pub rating: u8,
    pub current_tags: Vec<String>,
    // タグ候補カタログとピン留めタグは数十〜数百件になり得るうえ、overlay metadata は
    // フルスクリーン動画中 **毎フレーム** rebuild される (app::sync_native_video_metadata)。
    // 毎フレーム Vec を作り直すと UI スレッドで無駄な確保が積み上がるので、App 側で
    // 構築済み Arc をキャッシュし、ここはその Arc clone (refcount bump) だけにする。
    pub shortcut_tags: std::sync::Arc<[NativeOverlayTagDef]>,
    pub tag_choices: std::sync::Arc<[NativeOverlayTagDef]>,
    pub width: u32,
    pub height: u32,
    pub duration_secs: f64,
    pub video_codec: String,
    pub video_decoder: String,
    pub audio_codec: Option<String>,
    /// 音声ストリーム単体の平均ビットレート (bps)。0 のときは未知。
    pub audio_bit_rate_bps: i64,
    pub avg_fps: f64,
    pub bit_rate_bps: i64,
    pub chapter_count: usize,
    pub hw_decode_active: bool,
    pub gpu_path_active: bool,
    pub d3d11va_supported: bool,
    /// open 時に確定したデインターレースモード (Auto/On/Off)。
    pub deinterlace_mode: crate::settings::VideoDeinterlaceMode,
    /// 直近フレームのプレゼン経路 (動的)。
    pub last_present_path: crate::video::decoder::PresentPathSnapshot,
    /// bwdif フィルタの現在状態 (動的)。
    pub deinterlace_status: crate::video::decoder::DeinterlaceStatusSnapshot,
    /// 再生中に一度でもインターレースが検出されたか (latched、動的)。
    pub interlace_detected: bool,
    pub touch_video_chrome_learned: bool,
    pub panorama_detection: Option<
        Result<
            crate::video::spherical_metadata::VideoPanoramaTrigger,
            crate::video::spherical_metadata::VideoPanoramaRejection,
        >,
    >,
    pub shortcuts: NativeOverlayShortcutLabels,
    pub shortcut_help: Arc<NativeOverlayShortcutHelp>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NativeOverlayShortcutLabels {
    pub play_pause: Option<String>,
    pub seek_start: Option<String>,
    pub volume_up: Option<String>,
    pub volume_down: Option<String>,
    pub next_file: Option<String>,
    pub prev_file: Option<String>,
    pub mute: Option<String>,
    pub loop_mode: Option<String>,
    pub marker_prev: Option<String>,
    pub marker_next: Option<String>,
    pub pin: Option<String>,
    pub perf_overlay: Option<String>,
    pub window_mode: Option<String>,
    pub tile_mode: Option<String>,
    pub bookmark: Option<String>,
    pub capture: Option<String>,
    pub toggle_audio_mode: Option<String>,
    pub panorama: Option<String>,
    pub panorama_projection: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NativeOverlayShortcutHelp {
    pub sections: Vec<NativeOverlayShortcutHelpSection>,
    pub touch_rows: Vec<NativeOverlayShortcutHelpRow>,
    pub fixed_rows: Vec<NativeOverlayShortcutHelpRow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeOverlayShortcutHelpSection {
    pub title: String,
    pub rows: Vec<NativeOverlayShortcutHelpRow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeOverlayShortcutHelpRow {
    pub keys: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeOverlayVst3Panel {
    pub visible: bool,
    pub video_compact: bool,
    pub panel_pos: Option<[f32; 2]>,
    pub state_text: String,
    pub disabled_reason: Option<String>,
    pub slots: Vec<NativeOverlayVst3Slot>,
    pub chain_slots: Vec<NativeOverlayVst3ChainSlot>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeOverlayVst3Slot {
    pub idx: usize,
    pub path: String,
    pub name: String,
    pub state: NativeOverlayVst3SlotState,
    pub bypass: bool,
    pub gui_visible: bool,
    pub latency_ms: Option<f64>,
    pub auto_bypassed_for_latency: bool,
    pub placeholder: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeOverlayVst3SlotState {
    Loading,
    Loaded,
    Error,
    Placeholder,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeOverlayVst3ChainSlot {
    pub idx: usize,
    pub key_label: String,
    pub name: Option<String>,
    pub plugin_count: usize,
}

#[derive(Clone)]
pub struct NativeOverlayTileThumbnail {
    pub target_secs: f64,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
}

#[derive(Clone)]
pub struct NativeOverlaySeekStripCell {
    pub index: usize,
    pub content: NativeOverlaySeekStripCellContent,
}

#[derive(Clone)]
pub enum NativeOverlaySeekStripCellContent {
    Pending,
    Ready(NativeOverlayTileThumbnail),
    Failed { reason: &'static str },
}

#[derive(Clone)]
pub(crate) enum NativeOverlaySeekStripAxis {
    Resolving,
    Ready(Arc<crate::video::seek_strip::StripAxis>),
}

#[derive(Clone)]
pub struct NativeOverlaySeekStrip {
    pub center: crate::video::seek_strip::SeekStripCenter,
    pub(crate) axis: NativeOverlaySeekStripAxis,
    /// Persisted range shown in the fixed top-right label. This is deliberately separate from
    /// `waveform_span_secs`, which can temporarily retain the old raster scale during a rebuild.
    pub range_value_secs: f64,
    pub cell_count: usize,
    pub cells: Vec<NativeOverlaySeekStripCell>,
    pub thumbnail_notice: Option<NativeOverlaySeekStripThumbnailNotice>,
    pub wave_image: Option<NativeOverlaySeekStripWaveImage>,
    pub wave_notice: Option<NativeOverlaySeekStripWaveNotice>,
    pub waveform_span_secs: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeOverlaySeekStripThumbnailNotice {
    Unavailable,
}

impl NativeOverlaySeekStripThumbnailNotice {
    fn label(self) -> &'static str {
        match self {
            Self::Unavailable => "サムネイルを表示できません",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeOverlaySeekStripWaveNotice {
    NoAudioTrack,
    Failed,
}

impl NativeOverlaySeekStripWaveNotice {
    fn label(self) -> &'static str {
        match self {
            Self::NoAudioTrack => "この動画に音声はありません",
            Self::Failed => "波形を表示できません",
        }
    }
}

#[derive(Clone)]
pub struct NativeOverlaySeekStripWaveImage {
    pub window_start_secs: f64,
    pub window_end_secs: f64,
    pub bin_secs: f64,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
}

#[derive(Clone)]
pub struct NativeOverlayNavigationPreview {
    pub file_name: String,
    pub subtitle: String,
    pub thumbnail: Option<NativeOverlayTileThumbnail>,
}

#[derive(Clone)]
pub struct NativeOverlayTileOverlay {
    pub interval_secs: f64,
    pub timestamps: Vec<f64>,
    pub tile_w: u32,
    pub tile_h: u32,
    pub columns: usize,
    pub progress_done: usize,
    pub progress_total: usize,
    pub finished: bool,
    pub tiles: Vec<Option<NativeOverlayTileThumbnail>>,
    /// Keyboard cursor shown while tile mode is open. None is used for preparing overlays.
    pub selected_idx: Option<usize>,
    // ホイールで動画を切り替えた直後など metadata が None の数フレームでも、
    // 上部バーのタイトル行にファイル名を出すための fallback。
    pub fallback_file_name: String,
    // S タイル表示中の動画→動画 source swap では通常の center status HUD を
    // 描画しないため、動画オープン中の AVIO 進捗をタイル overlay 側で持つ。
    pub video_open_status: Option<crate::video::avio_progress::PreparingStatus>,
}

impl NativeOverlayTileOverlay {
    pub fn preparing() -> Self {
        Self::preparing_with_filename(String::new())
    }

    pub fn preparing_with_filename(file_name: String) -> Self {
        Self::preparing_with_open_status(
            file_name,
            crate::video::avio_progress::PreparingStatus {
                phase: crate::video::avio_progress::prep_phase::OPENING,
                bytes_read: 0,
                file_size: 0,
            },
        )
    }

    pub fn preparing_with_open_status(
        file_name: String,
        open_status: crate::video::avio_progress::PreparingStatus,
    ) -> Self {
        Self {
            interval_secs: 0.0,
            timestamps: Vec::new(),
            tile_w: 160,
            tile_h: 90,
            columns: 1,
            progress_done: 0,
            progress_total: 0,
            finished: false,
            tiles: Vec::new(),
            selected_idx: None,
            fallback_file_name: file_name,
            video_open_status: Some(open_status),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeOverlayRingPicker {
    pub title: String,
    pub rows: Vec<NativeOverlayRingPickerRow>,
    pub selected_row: Option<usize>,
    pub footer: String,
    pub drill: Option<NativeOverlayRingPickerDrill>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeOverlayRingPickerRow {
    pub label: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeOverlayRingPickerDrill {
    pub title: String,
    pub items: Vec<String>,
    pub selected: usize,
    pub footer: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeOverlayRingGuide {
    pub heading: String,
    pub detail: String,
    pub selected_slot: Option<usize>,
    pub center_client_px: Option<egui::Pos2>,
    pub slots: Vec<NativeOverlayRingGuideSlot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeOverlayRingGuideSlot {
    pub short_label: String,
    pub action_label: String,
}

#[derive(Clone, Debug)]
pub struct NativeOverlayJumpEntry {
    pub pts_secs: f64,
    pub kind: NativeOverlayTimelineMarkerKind,
    pub title: Option<String>,
    pub bookmark_id: Option<i64>,
    pub thumbnail: Option<NativeOverlayThumbnail>,
}

#[derive(Clone, Copy, Debug)]
struct NativePixelSample {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    format: i32,
    b: u8,
    g: u8,
    r: u8,
    a: u8,
}

struct SourceKeyedMutexAcquire {
    guard: Option<KeyedMutexReadGuard>,
    cast_ms: f64,
    acquire_ms: f64,
}

struct KeyedMutexReadGuard {
    mutex: IDXGIKeyedMutex,
    released_to_reader: Option<Arc<AtomicBool>>,
    armed: bool,
}

impl KeyedMutexReadGuard {
    /// Keep a successfully copied frame reusable by handing reader key 1
    /// directly back to the next presenter read. Once the frame is displaced
    /// and its pool slot becomes free, the producer observes
    /// `released_to_reader=true` and recovers writer key 0.
    fn release_to_reader(mut self) -> Result<(), String> {
        let result = unsafe { self.mutex.ReleaseSync(1) };
        if result.is_ok() {
            if let Some(released) = &self.released_to_reader {
                released.store(true, Ordering::Release);
            }
            self.armed = false;
        }
        result.map_err(|error| format!("source keyed mutex reader release failed: {error:?}"))
    }
}

impl Drop for KeyedMutexReadGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        unsafe {
            let _ = self.mutex.ReleaseSync(0);
        }
        if let Some(released) = &self.released_to_reader {
            released.store(false, Ordering::Release);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NativeOverlayInputRouting {
    pub wants_pointer_input: bool,
    pub wants_keyboard_input: bool,
    pub text_input_active: bool,
    /// ParkedLive の inert/dimmed HUD。ボタン機能は App 側 filter で実行されないため、
    /// HUD chrome 上の raw mouse button も復帰クリックとして App へ流してよい。
    pub hud_dimmed: bool,
    /// この egui パスが wheel イベントを `NavigateItem` / `TileColumnsDelta` コマンドへ
    /// 変換したか。true のとき同じ raw wheel イベントを `Window(MouseWheel)` として App へ
    /// 二重転送しない (= overlay コマンドと App 側 wheel ハンドラの二重適用を防ぐ)。
    /// タイルグリッドが `Order::Background` だと grid の余白上で egui の
    /// `wants_pointer_input()` が false になり、これが無いと Ctrl+ホイールでの列数変更が
    /// 2 ステップ進んでしまう。
    pub consumed_wheel: bool,
    /// テキスト入力中央モーダル (一括ブックマーク登録 / 名称編集) が表示中。
    /// `true` の間、wheel / button / keyboard の raw event を App へ転送しない
    /// (= dark backdrop 上のクリック・ホイールが video 移動 / 右クリック close fullscreen
    ///   / B キーで個別ブックマーク追加などを誘発するのを防ぐ)。
    /// Codex レビュー C1/C2/C3 反映、2026-05-24。
    pub modal_dialog_active: bool,
}

pub struct NativeOverlayInputOutcome {
    pub routing: NativeOverlayInputRouting,
    pub commands: Vec<NativeOverlayCommand>,
    pub(crate) window_intents: Vec<NativeWindowIntent>,
    /// CP5 で計算した HUD interactive regions (= 物理ピクセル単位 RECT 集合)。
    /// 表示中の bar / panel / popup / hover thumbnail などの矩形を含む。
    /// activation zone は **含めない** (= 上下端の VST 入力を奪わないため、Codex 5 P1 #1)。
    /// `NativeRenderCore::handle_window_events` の戻り値受信側で
    /// `apply_hud_regions(&hud_regions)` を呼んで `SetWindowRgn` を更新する。
    pub hud_regions: Vec<RECT>,
}

impl NativeOverlayInputOutcome {
    fn empty() -> Self {
        Self {
            routing: NativeOverlayInputRouting::default(),
            commands: Vec::new(),
            window_intents: Vec::new(),
            hud_regions: Vec::new(),
        }
    }
}

/// OS clipboard から UTF-16 テキストを読み出す。空 / 失敗時は None。
/// text input ダイアログ (一括ブックマーク登録 / bookmark title 編集) の Ctrl+V から呼ぶ。
fn read_clipboard_text_windows() -> Option<String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows::Win32::System::Ole::CF_UNICODETEXT;

    unsafe {
        if OpenClipboard(Some(HWND::default())).is_err() {
            return None;
        }
        let hmem = match GetClipboardData(CF_UNICODETEXT.0 as u32) {
            Ok(h) => h,
            Err(_) => {
                let _ = CloseClipboard();
                return None;
            }
        };
        if hmem.is_invalid() {
            let _ = CloseClipboard();
            return None;
        }
        let global = windows::Win32::Foundation::HGLOBAL(hmem.0);
        let ptr = GlobalLock(global) as *const u16;
        if ptr.is_null() {
            let _ = CloseClipboard();
            return None;
        }
        let max_bytes = GlobalSize(global);
        let max_u16 = max_bytes / 2;
        let mut len = 0usize;
        while len < max_u16 && *ptr.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(ptr, len);
        let s = String::from_utf16_lossy(slice);
        let _ = GlobalUnlock(global);
        let _ = CloseClipboard();
        if s.is_empty() { None } else { Some(s) }
    }
}

/// OS clipboard へ UTF-16 テキストを書き込む。
/// egui の `OutputCommand::CopyText` (= Ctrl+C / Ctrl+X の応答) から呼ぶ。
///
/// 失敗経路では必ず `GlobalFree(hmem)` を呼ぶ (Codex P3 2026-05-24):
/// clipboard が ownership を取るのは `SetClipboardData` が **成功** したときだけ。
/// それ以前のどの段階で抜けても解放しないと leak する。
fn write_clipboard_text_windows(text: &str) {
    use windows::Win32::Foundation::{GlobalFree, HANDLE, HWND};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{
        GLOBAL_ALLOC_FLAGS, GlobalAlloc, GlobalLock, GlobalUnlock,
    };
    use windows::Win32::System::Ole::CF_UNICODETEXT;

    let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = utf16.len() * 2;
    unsafe {
        let Ok(hmem) = GlobalAlloc(GLOBAL_ALLOC_FLAGS(0x0002), bytes) else {
            return;
        };
        let ptr = GlobalLock(hmem) as *mut u16;
        if ptr.is_null() {
            let _ = GlobalFree(Some(hmem));
            return;
        }
        std::ptr::copy_nonoverlapping(utf16.as_ptr(), ptr, utf16.len());
        let _ = GlobalUnlock(hmem);
        if OpenClipboard(Some(HWND::default())).is_err() {
            let _ = GlobalFree(Some(hmem));
            return;
        }
        let _ = EmptyClipboard();
        // SetClipboardData が成功するとメモリの ownership は OS clipboard に移る。
        // 失敗時はこちらが解放責任を持つので明示的に GlobalFree する。
        match SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(hmem.0))) {
            Ok(_) => {}
            Err(_) => {
                let _ = GlobalFree(Some(hmem));
            }
        }
        let _ = CloseClipboard();
    }
}

/// Windows clipboard (CF_UNICODETEXT) の CRLF / 単独 CR を LF に正規化する。
/// `egui-winit` の paste 経路と挙動を揃え、TextEdit に `\r` が残らないようにする。
fn normalize_clipboard_newlines(s: &str) -> String {
    // \r\n → \n を先に変換し、残った単独 \r も \n に置き換える (古い Mac 形式対応)。
    s.replace("\r\n", "\n").replace('\r', "\n")
}

#[derive(Clone, Copy)]
struct NativeRightPanelVisibilityInputs {
    shortcut_help_open: bool,
    external_drag_in_progress: bool,
    vst3_panel_visible: bool,
    metadata_available: bool,
    video_speed_popup_open: bool,
    hover_preview_active: bool,
    tag_picker_open: bool,
    pointer_in_hover_rect: bool,
    /// 360 度表示中は左右パネルを出さない。見回しドラッグで画面端まで指 / カーソルが
    /// 動くたびにパネルが開き、操作の邪魔になる (2026-08-27 利用者報告)。
    panorama_active: bool,
    side_panel_mode: FsSidePanelMode,
    info_panel_open: crate::ui_helpers::MetadataPanelOpenState,
}

fn native_right_panel_visible_from_inputs(input: NativeRightPanelVisibilityInputs) -> bool {
    if input.shortcut_help_open {
        return false;
    }
    if input.panorama_active {
        return false;
    }
    if input.external_drag_in_progress || input.vst3_panel_visible {
        return false;
    }
    if !input.metadata_available {
        return false;
    }
    if input.video_speed_popup_open || input.hover_preview_active {
        return false;
    }
    let explicit = crate::ui_helpers::metadata_panel_explicit_shown(
        input.side_panel_mode,
        input.info_panel_open,
    );
    input.tag_picker_open
        || match input.side_panel_mode.normalized() {
            FsSidePanelMode::Hover => input.pointer_in_hover_rect || explicit,
            FsSidePanelMode::ClickToShow => explicit,
            FsSidePanelMode::Unknown => unreachable!("normalized side panel mode"),
        }
}

#[derive(Clone, Copy)]
struct NativeJumpPanelVisibilityInputs {
    shortcut_help_open: bool,
    vst3_panel_visible: bool,
    video_speed_popup_open: bool,
    hover_preview_active: bool,
    pointer_in_hover_rect: bool,
    /// 右パネルと同じ理由で 360 度表示中は出さない。
    panorama_active: bool,
    side_panel_mode: FsSidePanelMode,
    left_panel_open: crate::ui_helpers::MetadataPanelOpenState,
}

fn native_jump_panel_visible_from_inputs(input: NativeJumpPanelVisibilityInputs) -> bool {
    if input.shortcut_help_open {
        return false;
    }
    if input.panorama_active {
        return false;
    }
    if input.vst3_panel_visible {
        return false;
    }
    if input.video_speed_popup_open || input.hover_preview_active {
        return false;
    }
    match input.side_panel_mode.normalized() {
        FsSidePanelMode::Hover => {
            input.pointer_in_hover_rect
                || input.left_panel_open == crate::ui_helpers::MetadataPanelOpenState::ByTouchHandle
        }
        FsSidePanelMode::ClickToShow => input.left_panel_open.is_open(),
        FsSidePanelMode::Unknown => unreachable!("normalized side panel mode"),
    }
}

#[cfg(test)]
mod clipboard_normalize_tests {
    use super::normalize_clipboard_newlines;

    #[test]
    fn crlf_becomes_lf() {
        assert_eq!(normalize_clipboard_newlines("a\r\nb\r\nc"), "a\nb\nc");
    }

    #[test]
    fn lone_cr_becomes_lf() {
        assert_eq!(normalize_clipboard_newlines("a\rb\rc"), "a\nb\nc");
    }

    #[test]
    fn lf_is_preserved() {
        assert_eq!(normalize_clipboard_newlines("a\nb\nc"), "a\nb\nc");
    }

    #[test]
    fn mixed_endings() {
        assert_eq!(normalize_clipboard_newlines("a\r\nb\nc\rd"), "a\nb\nc\nd");
    }

    #[test]
    fn no_newlines_unchanged() {
        assert_eq!(normalize_clipboard_newlines("plain text"), "plain text");
    }

    #[test]
    fn empty_unchanged() {
        assert_eq!(normalize_clipboard_newlines(""), "");
    }
}

fn native_panel_callout_hud_rects(
    width: f32,
    height: f32,
    left_visible: bool,
    right_visible: bool,
    vst_visible: bool,
) -> [Option<egui::Rect>; 2] {
    if vst_visible {
        return [None, None];
    }
    [
        left_visible.then(|| native_panel_callout_bar_rect(width, height, true)),
        right_visible.then(|| native_panel_callout_bar_rect(width, height, false)),
    ]
}

#[derive(Clone, Copy)]
struct NativeTouchPanelHandleInputs {
    chrome_latched: bool,
    blocked: bool,
    left_panel_open: bool,
    right_panel_open: bool,
    right_panel_available: bool,
}

fn native_touch_panel_handle_hud_rects(
    width: f32,
    height: f32,
    input: NativeTouchPanelHandleInputs,
) -> [Option<egui::Rect>; 2] {
    if !input.chrome_latched || input.blocked {
        return [None, None];
    }
    let left = (!input.left_panel_open)
        .then(|| native_touch_panel_handle_rect(width, height, true))
        .filter(egui::Rect::is_positive);
    let right = (input.right_panel_available && !input.right_panel_open)
        .then(|| native_touch_panel_handle_rect(width, height, false))
        .filter(egui::Rect::is_positive);
    [left, right]
}

fn native_touch_panel_tap_command_dismisses_before_dispatch(
    command: crate::video::native_touch::NativeVideoTouchCommand,
    left_open: crate::ui_helpers::MetadataPanelOpenState,
    right_open: crate::ui_helpers::MetadataPanelOpenState,
) -> bool {
    matches!(
        command,
        crate::video::native_touch::NativeVideoTouchCommand::ToggleChrome
            | crate::video::native_touch::NativeVideoTouchCommand::SeekRelative { .. }
    ) && (left_open == crate::ui_helpers::MetadataPanelOpenState::ByTouchHandle
        || right_open == crate::ui_helpers::MetadataPanelOpenState::ByTouchHandle)
}

fn native_touch_owned_panel_sides(
    left_open: crate::ui_helpers::MetadataPanelOpenState,
    right_open: crate::ui_helpers::MetadataPanelOpenState,
) -> [bool; 2] {
    [
        left_open == crate::ui_helpers::MetadataPanelOpenState::ByTouchHandle,
        right_open == crate::ui_helpers::MetadataPanelOpenState::ByTouchHandle,
    ]
}

fn native_video_fullscreen_shortcut_key(
    key: &crate::video::native_window::NativeVideoKeyEvent,
) -> bool {
    crate::keymap::native_video_fullscreen_shortcut_key(key)
}

#[derive(Clone, Debug)]
pub enum NativeOverlayCommand {
    Seek {
        target_secs: f64,
    },
    SeekRelative {
        delta_secs: f64,
    },
    TouchChromeLearned,
    PanoramaDrag {
        delta_points: egui::Vec2,
        viewport_height_points: f32,
    },
    PanoramaWheel {
        delta: i32,
    },
    TogglePanorama,
    CyclePanoramaProjection,
    ResetPanorama,
    TileSeek {
        target_secs: f64,
    },
    NavigateItem {
        delta: i32,
        via_wheel: bool,
    },
    TileColumnsDelta {
        delta: i32,
    },
    RequestSeekThumbnail {
        target_secs: f64,
        bar_width_points: f64,
        pixels_per_point: f64,
    },
    /// hover が外れて seek thumbnail 要求がもう不要 (T35)。
    /// Player 側で `clear_native_hover_thumbnail` を呼んで pump の永久リトライを止める。
    ClearSeekThumbnail,
    OpenSeekStrip,
    CloseSeekStrip {
        cause: crate::video::seek_strip::SeekStripCloseCause,
    },
    MoveSeekStrip {
        center: crate::video::seek_strip::SeekStripCenter,
    },
    CommitSeekStrip {
        center: crate::video::seek_strip::SeekStripCenter,
    },
    RequestSeekStripWindow {
        center: crate::video::seek_strip::SeekStripCenter,
        visible_count: usize,
        pixel_width: usize,
        pixel_height: usize,
    },
    SetSeekStripState {
        state: crate::settings::VideoSeekStripState,
    },
    StepSeekStripRange {
        step: crate::video::seek_strip::SeekStripRangeStep,
    },
    ToggleTileMode,
    TogglePerfOverlay,
    ToggleSidePanelMode,
    ToggleBarLock {
        bar: crate::video::NativeVideoBar,
    },
    ToggleSeekStripLock,
    ToggleClickInfoOpen,
    OpenTouchInfoPanel,
    DismissTouchSidePanels,
    ToggleVst3Gui,
    /// 動画 HUD の「音声モード」ボタン: 映像を切って音楽ビュー (DJ 波形 + spectrum) へ切り替える
    /// (Inc 7、動画→音声モード)。App が `enter_video_audio_mode` を呼ぶ。音声は無中断。
    ToggleAudioMode,
    CloseFullscreen,
    /// 動画 HUD のトグルボタン: ウィンドウ内再生 ⇔ 全画面 を切り替える。
    ToggleWindowMode,
    SetVst3PanelVisible {
        visible: bool,
    },
    SetVst3VideoCompact {
        compact: bool,
    },
    SetVst3PanelPos {
        pos: [f32; 2],
    },
    Vst3ShowSlotGui {
        idx: usize,
        path: String,
    },
    Vst3HideSlotGui {
        idx: usize,
        path: String,
    },
    Vst3SetBypass {
        idx: usize,
        path: String,
        bypass: bool,
    },
    Vst3LoadChainSlot {
        slot_idx: usize,
    },
    Vst3SaveChainSlot {
        slot_idx: usize,
    },
    VideoAdjustLoadSlot {
        slot_idx: usize,
    },
    VideoAdjustSaveSlot {
        slot_idx: usize,
    },
    SeekToStartAndPlay,
    TogglePlay,
    ToggleMute,
    SetVolume {
        volume: f64,
        persist: bool,
    },
    SetVideoAdjustments {
        adjustments: crate::creative_lut::VideoAdjustments,
        persist: bool,
    },
    RequestVideoScaleSettings {
        filter: VideoScaleFilter,
        smoothing_percent: u32,
    },
    SetVideoAnime4kBudget {
        budget: crate::video::anime4k_policy::VideoAnime4kBudgetPreset,
    },
    RemeasureVideoAnime4k,
    SetPlaybackSpeed {
        speed: f64,
    },
    CopyFrameToClipboard,
    FrameStep {
        direction: i32,
    },
    ToggleLoop,
    ToggleContinuous,
    AddBookmarkAt {
        target_secs: f64,
    },
    SetPinAt {
        target_secs: f64,
    },
    /// 動画 HUD 2 段化リデザイン (Phase 4): 前/次マーカー (chapter / bookmark / pin) へジャンプ。
    /// `next=true` で次マーカー (= K キー)、`false` で前マーカー (= J キー)。
    /// App 側 dispatch は `jump_native_video_marker(fs_idx, next)`。
    JumpMarker {
        next: bool,
    },
    /// 動画 HUD 2 段化リデザイン (Phase 5): 現在フレームをキャプチャ保存フォルダへ保存
    /// (= Ctrl+S と等価)。App 側 dispatch は `save_video_frame_to_file(ctx, fs_idx)`。
    SaveFrameToFile,
    SetBookmarkTitle {
        id: i64,
        title: String,
    },
    DeleteBookmark {
        id: i64,
    },
    DeletePin,
    /// 一括ブックマーク登録 (YouTube コメント形式のチャプター列を一括追加)。
    /// 重複は ±1s で skip、結果はトーストで通知。
    BulkAddBookmarks {
        entries: Vec<(f64, String)>,
    },
    /// 現在再生中の動画のブックマーク一覧をクリップボードへコピー。
    /// `seconds_only` が true なら整数秒に floor、false なら小数 3 桁 (= ms 精度) で出力。
    ExportBookmarksToClipboard {
        seconds_only: bool,
    },
    /// 現在再生中の動画のブックマークを全削除。
    ClearAllBookmarksForCurrent,
    OpenExternalUrl {
        url: String,
    },
    /// 右パネル先頭の★行クリック。解決済みの新レーティング (0..=5、同★再クリックで 0)。
    SetRating {
        stars: u8,
    },
    ToggleTag {
        name: String,
    },
    AddTag {
        name: String,
    },
    RemoveTag {
        name: String,
    },
    OpenTagViewForTag {
        name: String,
    },
    /// 音量ノーマライズボタンの左クリック (3 状態モデルでトグル動作)。
    ToggleNormalize,
    /// 音量ノーマライズボタンの右クリック (どの状態からでもグローバル OFF 化)。
    DisableNormalize,
    /// スキャン中の進捗パネル × / ESC でキャンセル。
    CancelNormalizeScan,
}

impl NativeOverlayInputRouting {
    pub fn should_forward_to_ui(
        self,
        event: &crate::video::native_window::NativeVideoWindowEvent,
    ) -> bool {
        use crate::video::native_window::NativeVideoWindowEvent as NativeEvent;

        match event {
            NativeEvent::KeyDown(key) | NativeEvent::KeyUp(key) => {
                if self.text_input_active {
                    // テキスト入力中 (タグピッカー / ブックマーク編集) はキーをテキスト編集が
                    // 消費するので App へ一切転送しない。`wants_keyboard_input` (前フレーム
                    // sample) は TextEdit が確定 Enter でフォーカスを一瞬失うとその隙に false に
                    // なり、Enter が App へ漏れて fullscreen close = 再生停止を誘発していた
                    // (動画タグ付与で Enter 確定 → 停止、の実害)。text_input_active で確実に塞ぐ。
                    false
                } else if self.modal_dialog_active {
                    // モーダル中は動画ショートカットを含めて App へ流さない
                    // (B キーでブックマーク追加、Space で再生トグル等の暴発防止)。
                    false
                } else if native_video_fullscreen_shortcut_key(key) {
                    true
                } else {
                    !self.wants_keyboard_input
                }
            }
            NativeEvent::Text(_) | NativeEvent::Ime(_) => {
                !self.wants_keyboard_input && !self.modal_dialog_active
            }
            // overlay が wheel を NavigateItem / TileColumnsDelta に変換済みなら、
            // 同じ raw wheel を App へ二重転送しない。
            // モーダル中はカーソルがダイアログ外の dark backdrop にあっても wheel を
            // App へ流さない (= 動画切替誘発防止、Codex P3 C1)。
            NativeEvent::MouseWheel(_) => {
                if self.modal_dialog_active || self.consumed_wheel {
                    false
                } else {
                    !self.wants_pointer_input
                }
            }
            // モーダル中はクリックも App へ流さない (= 右クリックで fullscreen 終了して
            // 入力中テキストが消える事故防止、Codex P2 C3)。
            NativeEvent::MouseButton(button)
                if button.button == crate::video::native_window::NativeVideoMouseButton::Right =>
            {
                self.hud_dimmed || !self.modal_dialog_active
            }
            NativeEvent::MouseMove(_) | NativeEvent::MouseLeave => {
                !self.wants_pointer_input && !self.modal_dialog_active
            }
            NativeEvent::MouseButton(_) => {
                self.hud_dimmed || (!self.wants_pointer_input && !self.modal_dialog_active)
            }
            // native viewer の close は App 側でセッション終了として扱う。
            NativeEvent::CloseRequested { .. } => true,
            // Touch is consumed by the presenter-local recognizer and primary
            // emulation. It must never re-enter the legacy App mouse path.
            NativeEvent::Touch(_) => false,
            // 内部処理イベント (presenter thread が直接消費する)。UI 転送しない。
            NativeEvent::GeometryChanged { .. }
            | NativeEvent::DpiChanged { .. }
            | NativeEvent::RequestRaiseHud
            | NativeEvent::RequestFocusClaim
            | NativeEvent::CursorOwnership(_)
            | NativeEvent::Destroyed => false,
        }
    }
}

fn create_present_d3d11_device() -> Result<(ID3D11Device, ID3D11DeviceContext), String> {
    let mut device = None;
    let mut context = None;
    let mut feature_level = D3D_FEATURE_LEVEL::default();
    let feature_levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
    let flags = D3D11_CREATE_DEVICE_FLAG(D3D11_CREATE_DEVICE_BGRA_SUPPORT.0);
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            windows::Win32::Foundation::HMODULE::default(),
            flags,
            Some(&feature_levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            Some(&mut feature_level),
            Some(&mut context),
        )
        .map_err(|e| format!("D3D11CreateDevice presenter: {e:?}"))?;
    }
    let device =
        device.ok_or_else(|| "D3D11CreateDevice presenter returned null device".to_string())?;
    let context =
        context.ok_or_else(|| "D3D11CreateDevice presenter returned null context".to_string())?;
    crate::logger::log(format!(
        "native-presenter: presenter D3D11 device created (feature_level=0x{:X})",
        feature_level.0
    ));
    Ok((device, context))
}

fn configure_overlay_fonts(ctx: &egui::Context, ui_font: &crate::settings::UiFontSettings) {
    crate::ui_fonts::configure_fonts_with_settings(ctx, ui_font);
}

fn configure_overlay_style(ctx: &egui::Context, text_contrast: crate::settings::TextContrast) {
    crate::os_theme::apply_resolved_with_contrast(
        ctx,
        crate::os_theme::ResolvedTheme::Dark,
        text_contrast,
    );
    ctx.style_mut(|style| {
        // native HUD は HUD_BOTTOM_HEIGHT (= 64pt、2 段) の小さな操作面なので、ヘルプ
        // text は hover 直後に出す。egui default の 0.5s delay だと「初回だけ待つ /
        // 隣ボタンは即時」という grace-time 由来の不揃いな挙動に見える。
        style.interaction.tooltip_delay = 0.0;
    });
}

/// CP9 実機 debug: `MIV_HUD_DEBUG=1` で起動したか。
/// 起動後一度評価された値を cache (= env を変えても再評価しない、Once セマンティクス)。
pub(crate) fn hud_debug_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("MIV_HUD_DEBUG").is_some())
}

fn hud_repaint_debug_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("MIV_HUD_DEBUG_REPAINT").is_some())
}

impl NativeRenderCore {
    pub fn anime4k_adapter_key(
        &self,
    ) -> Result<crate::video::anime4k_policy::VideoAnime4kAdapterKey, String> {
        let (_, key) = self.anime4k_adapter_and_key()?;
        Ok(key)
    }

    pub fn start_anime4k_measurement(&self) -> Result<NativeAnime4kMeasurement, String> {
        let (adapter, key) = self.anime4k_adapter_and_key()?;
        let (tx, updates) = crossbeam_channel::bounded(8);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        std::thread::Builder::new()
            .name("video-anime4k-measure".to_string())
            .spawn(move || {
                let result = measure_video_anime4k_on_adapter(
                    &adapter,
                    &worker_cancel,
                    |completed, point| {
                        let _ =
                            tx.send(NativeAnime4kMeasurementUpdate::Progress { completed, point });
                    },
                )
                .map(|points| {
                    crate::video::anime4k_policy::VideoAnime4kMeasurementCache {
                        schema: crate::video::anime4k_policy::VIDEO_ANIME4K_MEASUREMENT_SCHEMA,
                        adapter: key,
                        points,
                    }
                });
                let _ = tx.send(NativeAnime4kMeasurementUpdate::Completed(result));
            })
            .map_err(|error| format!("spawn Anime4K measurement worker: {error}"))?;
        Ok(NativeAnime4kMeasurement { updates, cancel })
    }

    fn anime4k_adapter_and_key(
        &self,
    ) -> Result<
        (
            IDXGIAdapter,
            crate::video::anime4k_policy::VideoAnime4kAdapterKey,
        ),
        String,
    > {
        let dxgi_device: IDXGIDevice = self
            .d3d_device1
            .cast()
            .map_err(|error| format!("Anime4K adapter IDXGIDevice: {error:?}"))?;
        let adapter = unsafe { dxgi_device.GetAdapter() }
            .map_err(|error| format!("Anime4K adapter GetAdapter: {error:?}"))?;
        let adapter1: IDXGIAdapter1 = adapter
            .cast()
            .map_err(|error| format!("Anime4K adapter IDXGIAdapter1: {error:?}"))?;
        let desc = unsafe { adapter1.GetDesc1() }
            .map_err(|error| format!("Anime4K adapter GetDesc1: {error:?}"))?;
        let name_len = desc
            .Description
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(desc.Description.len());
        let description = String::from_utf16_lossy(&desc.Description[..name_len]);
        let driver_version = unsafe { adapter.CheckInterfaceSupport(&ID3D11Device::IID) }
            .map(|value| value as u64)
            .unwrap_or(0);
        Ok((
            adapter,
            crate::video::anime4k_policy::VideoAnime4kAdapterKey {
                luid_low: desc.AdapterLuid.LowPart,
                luid_high: desc.AdapterLuid.HighPart,
                vendor_id: desc.VendorId,
                device_id: desc.DeviceId,
                subsystem_id: desc.SubSysId,
                revision: desc.Revision,
                driver_version,
                dedicated_video_memory: desc.DedicatedVideoMemory as u64,
                shared_system_memory: desc.SharedSystemMemory as u64,
                description,
            },
        ))
    }

    pub(crate) fn new(
        config: NativeRenderConfig,
    ) -> Result<(Self, HostWindowTopology, Vec<NativeWindowIntent>), String> {
        let health = Arc::clone(&config.health);
        let window_epoch = config.window_epoch;
        let _attach_operation = health.begin_render_operation(
            crate::video::native_window_health::NativeRenderOperation::Attach,
            window_epoch,
        );
        // placement 切替 (F12) はこの関数を丸ごと走らせる。どの段が支配的かを
        // 1 切替につき 1 行だけ残す (backlog §1.122)。
        let core_t0 = Instant::now();
        unsafe {
            let (d3d_device, d3d_context) = create_present_d3d11_device()?;
            let d3d_device1: ID3D11Device1 = d3d_device
                .cast()
                .map_err(|e| format!("cast ID3D11Device1: {e:?}"))?;
            let d3d_device5: ID3D11Device5 = d3d_device
                .cast()
                .map_err(|e| format!("cast ID3D11Device5: {e:?}"))?;
            let d3d_context4: ID3D11DeviceContext4 = d3d_context
                .cast()
                .map_err(|e| format!("cast ID3D11DeviceContext4: {e:?}"))?;
            let d3d_context1: ID3D11DeviceContext1 = d3d_context
                .cast()
                .map_err(|e| format!("cast ID3D11DeviceContext1: {e:?}"))?;
            let d3d_ms = core_t0.elapsed().as_secs_f64() * 1000.0;
            let pipelines_t0 = Instant::now();
            // Compile every small Phase A shader while the render pipeline is
            // created. Anime4K pixel shaders are loaded here from build-time FXC
            // bytecode. A later filter selection never invokes D3DCompile.
            let grade_pipeline = VideoGradePipeline::new(&d3d_device1)?;
            let (panorama_pipeline, panorama_pipeline_error) =
                match VideoPanoramaPipeline::new(&d3d_device1) {
                    Ok(pipeline) => (Some(pipeline), None),
                    Err(error) => {
                        crate::logger::log(format!(
                            "native-presenter: video panorama pipeline unavailable: {error}"
                        ));
                        (None, Some(error))
                    }
                };
            let (resample_pipeline, resample_pipeline_error) = match VideoResamplePipeline::new(
                &d3d_device1,
            ) {
                Ok(pipeline) => (Some(pipeline), None),
                Err(error) => {
                    crate::logger::log(format!(
                        "native-presenter: standard video resample pipeline unavailable: {error}"
                    ));
                    (None, Some(error))
                }
            };
            let pipelines_ms = pipelines_t0.elapsed().as_secs_f64() * 1000.0;
            let compositor_t0 = Instant::now();
            let dxgi_device: IDXGIDevice = d3d_device
                .cast()
                .map_err(|e| format!("cast IDXGIDevice: {e:?}"))?;
            let adapter = dxgi_device
                .GetAdapter()
                .map_err(|e| format!("IDXGIDevice::GetAdapter: {e:?}"))?;
            let factory: IDXGIFactory2 = adapter
                .GetParent()
                .map_err(|e| format!("IDXGIAdapter::GetParent: {e:?}"))?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: config.width,
                Height: config.height,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                Stereo: false.into(),
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 3,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                Flags: DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32,
            };
            let swap_chain = factory
                .CreateSwapChainForComposition(&d3d_device, &desc, None::<&IDXGIOutput>)
                .map_err(|e| format!("CreateSwapChainForComposition: {e:?}"))?;
            let swap_chain2: IDXGISwapChain2 = swap_chain
                .cast()
                .map_err(|e| format!("cast IDXGISwapChain2: {e:?}"))?;
            swap_chain2
                .SetMaximumFrameLatency(1)
                .map_err(|e| format!("SetMaximumFrameLatency: {e:?}"))?;
            let waitable = swap_chain2.GetFrameLatencyWaitableObject();

            let dcomp_device: IDCompositionDevice = DCompositionCreateDevice(&dxgi_device)
                .map_err(|e| format!("DCompositionCreateDevice: {e:?}"))?;
            let presenter_target = config.targets.presenter();
            let target = presenter_target
                .create_dcomp_target(&dcomp_device)
                .map_err(|e| format!("CreateTargetForHwnd: {e:?}"))?;
            let root_visual = dcomp_device
                .CreateVisual()
                .map_err(|e| format!("CreateVisual root: {e:?}"))?;
            let background = NativeBlackBackground::new(
                &factory,
                &d3d_device,
                &d3d_device1,
                &d3d_context,
                &dcomp_device,
                config.width,
                config.height,
            )?;
            root_visual
                .AddVisual(&background._visual, false, None::<&IDCompositionVisual>)
                .map_err(|e| format!("IDCompositionVisual::AddVisual background: {e:?}"))?;
            let video_visual = dcomp_device
                .CreateVisual()
                .map_err(|e| format!("CreateVisual video: {e:?}"))?;
            video_visual
                .SetContent(&swap_chain)
                .map_err(|e| format!("IDCompositionVisual::SetContent video: {e:?}"))?;
            root_visual
                .AddVisual(&video_visual, true, &background._visual)
                .map_err(|e| format!("IDCompositionVisual::AddVisual video: {e:?}"))?;
            // The host creates and owns any HUD HWND before render initialization.  The
            // core receives only opaque target leases and keeps DComp resources.
            let mut active_topology = config.targets.topology();
            let mut hud_dcomp_target: Option<IDCompositionTarget> = None;
            let mut hud_root_visual: Option<IDCompositionVisual> = None;
            let mut startup_window_intents = Vec::new();
            if let Some(hud_target) = config.targets.hud() {
                match hud_target.create_dcomp_target(&dcomp_device) {
                    Ok(target) => match dcomp_device.CreateVisual() {
                        Ok(root_for_hud) => match target.SetRoot(&root_for_hud) {
                            Ok(()) => {
                                hud_dcomp_target = Some(target);
                                hud_root_visual = Some(root_for_hud);
                                crate::logger::log(
                                    "[native-video] HUD overlay HWND attached to render core"
                                        .to_string(),
                                );
                            }
                            Err(err) => {
                                active_topology = HostWindowTopology::PresenterOnly;
                                crate::logger::log(format!(
                                    "native-presenter: HUD DComp target SetRoot failed, fallback: {err:?}"
                                ));
                            }
                        },
                        Err(err) => {
                            active_topology = HostWindowTopology::PresenterOnly;
                            crate::logger::log(format!(
                                "native-presenter: HUD DComp CreateVisual failed, fallback: {err:?}"
                            ));
                        }
                    },
                    Err(err) => {
                        active_topology = HostWindowTopology::PresenterOnly;
                        crate::logger::log(format!(
                            "native-presenter: HUD CreateTargetForHwnd failed, fallback: {err:?}"
                        ));
                    }
                }
            }

            let compositor_ms = compositor_t0.elapsed().as_secs_f64() * 1000.0;
            let overlay_t0 = Instant::now();
            let mut overlay_new_ms = 0.0;
            let mut overlay_first_render_ms = 0.0;

            // CP4 + P2 #1 反映: egui overlay の attach は HUD 経路 → presenter フォールバック
            // 経路の 2 段階で試す。HUD 経路で `NativeEguiOverlay::new` が失敗したら、
            // HUD fields を全部 None にクリアしてから presenter フォールバック経路で
            // retry する。これがないと「HUD HWND は作られたが overlay は disable」という
            // 中途半端な状態 (= `hud_hwnd_out` に非 0、bars が見えない) になる。
            let mut egui_overlay: Option<NativeEguiOverlay> = None;
            if config.egui_overlay {
                // Step 1: HUD 経路で attach を試みる。
                if active_topology == HostWindowTopology::PresenterAndHud
                    && let Some(hud_root) = hud_root_visual.as_ref()
                {
                    let hud_target = config
                        .targets
                        .hud()
                        .expect("HUD topology must carry a HUD target");
                    let overlay_visual = dcomp_device
                        .CreateVisual()
                        .map_err(|e| format!("CreateVisual egui overlay (HUD): {e:?}"))?;
                    let overlay_new_t0 = Instant::now();
                    let overlay_result = NativeEguiOverlay::new(
                        overlay_visual,
                        &dcomp_device,
                        hud_root,
                        None,
                        hud_target,
                        config.width,
                        config.height,
                        config.os_pixels_per_point,
                        config.initial_observation,
                        config.cursor_hide_delay_secs,
                        config.ui_scale,
                        config.text_contrast,
                        config.ui_font.clone(),
                        Arc::clone(&health),
                        window_epoch,
                    );
                    overlay_new_ms += overlay_new_t0.elapsed().as_secs_f64() * 1000.0;
                    match overlay_result {
                        Ok(mut overlay) => {
                            let first_render_t0 = Instant::now();
                            match overlay.render_once() {
                                Ok((_, intents)) => startup_window_intents.extend(intents),
                                Err(err) => {
                                    crate::logger::log(format!(
                                        "native-presenter: HUD egui overlay initial render failed: {err}"
                                    ));
                                    log_event(
                                        "egui_overlay_error",
                                        &[("error", Value::from(err.to_string()))],
                                    );
                                }
                            }
                            overlay_first_render_ms +=
                                first_render_t0.elapsed().as_secs_f64() * 1000.0;
                            egui_overlay = Some(overlay);
                        }
                        Err(err) => {
                            // HUD 経路で失敗 → HUD fields を全部 drop して presenter フォールバック retry。
                            // `hud_window` / `hud_dcomp_target` / `hud_root_visual` / `hud_regions` を
                            // None にすることで HUD HWND が destroy され、`hud_hwnd_out` も 0 のままになる。
                            crate::logger::log(format!(
                                "native-presenter: HUD egui overlay init failed, falling back to presenter DComp tree: {err}"
                            ));
                            log_event(
                                "egui_overlay_error",
                                &[
                                    ("error", Value::from(err.to_string())),
                                    ("phase", Value::from("hud_path_fallback")),
                                ],
                            );
                            active_topology = HostWindowTopology::PresenterOnly;
                            hud_dcomp_target = None;
                            hud_root_visual = None;
                        }
                    }
                }

                // Step 2: フォールバック経路。Step 1 が完全成功なら skip、HUD HWND なし or
                // HUD 経路で失敗した場合は presenter root の DComp tree に attach する。
                if egui_overlay.is_none() {
                    let overlay_visual = dcomp_device
                        .CreateVisual()
                        .map_err(|e| format!("CreateVisual egui overlay (fallback): {e:?}"))?;
                    let overlay_new_t0 = Instant::now();
                    let overlay_result = NativeEguiOverlay::new(
                        overlay_visual,
                        &dcomp_device,
                        &root_visual,
                        Some(&video_visual),
                        presenter_target,
                        config.width,
                        config.height,
                        config.os_pixels_per_point,
                        config.initial_observation,
                        config.cursor_hide_delay_secs,
                        config.ui_scale,
                        config.text_contrast,
                        config.ui_font.clone(),
                        Arc::clone(&health),
                        window_epoch,
                    );
                    overlay_new_ms += overlay_new_t0.elapsed().as_secs_f64() * 1000.0;
                    match overlay_result {
                        Ok(mut overlay) => {
                            let first_render_t0 = Instant::now();
                            match overlay.render_once() {
                                Ok((_, intents)) => startup_window_intents.extend(intents),
                                Err(err) => {
                                    crate::logger::log(format!(
                                        "native-presenter: egui overlay initial render failed: {err}"
                                    ));
                                    log_event(
                                        "egui_overlay_error",
                                        &[("error", Value::from(err.to_string()))],
                                    );
                                }
                            }
                            overlay_first_render_ms +=
                                first_render_t0.elapsed().as_secs_f64() * 1000.0;
                            egui_overlay = Some(overlay);
                        }
                        Err(err) => {
                            crate::logger::log(format!(
                                "native-presenter: egui overlay disabled after init failure: {err}"
                            ));
                            log_event(
                                "egui_overlay_error",
                                &[("error", Value::from(err.to_string()))],
                            );
                        }
                    }
                }
            }

            let overlay_ms = overlay_t0.elapsed().as_secs_f64() * 1000.0;
            crate::logger::log(format!(
                "native-presenter: render core created in {:.1}ms \
                 (d3d11_device={d3d_ms:.1} shader_pipelines={pipelines_ms:.1} \
                 swapchain_dcomp={compositor_ms:.1} egui_overlay={overlay_ms:.1} \
                 overlay_new={overlay_new_ms:.1} \
                 overlay_first_render={overlay_first_render_ms:.1}) \
                 {}x{} epoch={window_epoch}",
                core_t0.elapsed().as_secs_f64() * 1000.0,
                config.width,
                config.height,
            ));

            // Route A — MPO ちらつき修正 (v0.9.2): 動画 visual の真上に完全透明な
            // 全画面カバー visual を常駐させる。presenter HWND の内容が「動画 swap
            // chain 単独」でなくなり、DWM が動画をハードウェアオーバーレイ面 (MPO)
            // へ昇格できなくなる。これにより、トースト / HUD 出現時の MPO プレーン
            // 降格遷移で動画フレームが 1 フレームずれて見える不具合が解消する。
            // カバーは premultiplied alpha の (0,0,0,0) なので表示は一切変わらない。
            // `MIV_DISABLE_MPO_COVER` で無効化できる (= 万一カバーが特定 GPU /
            // ドライバで問題を起こした場合のエスケープハッチ)。
            let mpo_cover_enabled = std::env::var_os("MIV_DISABLE_MPO_COVER").is_none();
            let test_overlay = if config.test_overlay && egui_overlay.is_none() {
                // デバッグ用の可視テストパターン (従来機能、明示指定時のみ)。
                let overlay = NativeTestOverlay::new(
                    &factory,
                    &d3d_device,
                    &d3d_device1,
                    &d3d_context,
                    &d3d_context1,
                    &dcomp_device,
                    config.width,
                    config.height,
                    false,
                )?;
                root_visual
                    .AddVisual(&overlay._visual, true, &video_visual)
                    .map_err(|e| format!("IDCompositionVisual::AddVisual overlay: {e:?}"))?;
                Some(overlay)
            } else if mpo_cover_enabled {
                // 本番: MPO 防止の透明カバー。
                let overlay = NativeTestOverlay::new(
                    &factory,
                    &d3d_device,
                    &d3d_device1,
                    &d3d_context,
                    &d3d_context1,
                    &dcomp_device,
                    config.width,
                    config.height,
                    true,
                )?;
                root_visual
                    .AddVisual(&overlay._visual, true, &video_visual)
                    .map_err(|e| format!("IDCompositionVisual::AddVisual mpo cover: {e:?}"))?;
                crate::logger::log(
                    "[native-video] MPO-defeat transparent cover enabled".to_string(),
                );
                Some(overlay)
            } else {
                None
            };
            target
                .SetRoot(&root_visual)
                .map_err(|e| format!("IDCompositionTarget::SetRoot: {e:?}"))?;
            dcomp_device
                .Commit()
                .map_err(|e| format!("IDCompositionDevice::Commit: {e:?}"))?;

            // presenter 自前の copy fence。フレームコピーの GPU 完了を `present_retire` 側で
            // 待ち合わせるために使う。作成失敗は致命的ではない (= `present_retire` が時間
            // ベース depth キャップへフォールバック) ので、Err にせず None で続行する。
            let copy_fence: Option<ID3D11Fence> = {
                let mut fence = None;
                match d3d_device5.CreateFence(0, D3D11_FENCE_FLAG_NONE, &mut fence) {
                    Ok(()) => fence,
                    Err(e) => {
                        crate::logger::log(format!(
                            "native-presenter: copy fence CreateFence failed ({e:?}); \
                             present_retire falls back to depth cap"
                        ));
                        None
                    }
                }
            };

            let overlay_composition_target =
                if active_topology == HostWindowTopology::PresenterAndHud {
                    NativeOverlayCompositionTarget::Hud {
                        _target: hud_dcomp_target
                            .take()
                            .expect("HUD topology must retain its DComp target"),
                        _root_visual: hud_root_visual
                            .take()
                            .expect("HUD topology must retain its root visual"),
                    }
                } else {
                    NativeOverlayCompositionTarget::Presenter
                };

            let mut this = Self {
                health: Arc::clone(&health),
                window_epoch,
                swap_chain,
                waitable,
                d3d_device1,
                d3d_device5,
                d3d_context,
                d3d_context1,
                d3d_context4,
                _dcomp_device: dcomp_device,
                _dcomp_target: target,
                _root_visual: root_visual,
                _background: background,
                _video_visual: video_visual,
                backbuffer: None,
                test_overlay,
                egui_overlay,
                window_observation: config.initial_observation,
                _overlay_composition_target: overlay_composition_target,
                lbutton_down_since: None,
                fence_cache: None,
                copy_fence,
                copy_fence_value: 0,
                shared_texture_cache: Vec::new(),
                cpu_upload_scratch: Vec::new(),
                video_grade: crate::creative_lut::VideoGradeSnapshot::default(),
                grade_pipeline,
                panorama_pipeline,
                panorama_pipeline_error,
                panorama_pose: None,
                resample_pipeline,
                resample_pipeline_error,
                selected_scale_filter: config.scale_filter,
                downscale_smoothing_percent: crate::settings::sanitize_downscale_smoothing_percent(
                    config.downscale_smoothing_percent,
                ),
                selected_anime4k_variant: config.anime4k_variant,
                anime4k_budget: config.anime4k_budget,
                anime4k_status: config.anime4k_status,
                surface_content: VideoSurfaceContent::LegacySource,
                resize_settle: VideoResizeSettleState::Settled,
                prepared_video_surface: None,
                prepared_video_scale_settings: None,
                active_scale_fallback: None,
                scale_fallback_count: 0,
                surface_swap_count: 0,
                pixel_probe_enabled: std::env::var_os("MIV_NATIVE_VIDEO_PIXEL_PROBE").is_some(),
                pixel_probe_strict: std::env::var_os("MIV_NATIVE_VIDEO_PIXEL_PROBE_STRICT")
                    .is_some(),
                last_pixel_probe: None,
                video_compact: false,
                video_top_bar_locked: false,
                video_bottom_lock: VideoBottomLock::None,
                fullscreen_fixed_bar_gap_px: 0,
                sar_num: 1,
                sar_den: 1,
                orientation: VideoOrientation::IDENTITY,
                width: config.width,
                height: config.height,
                surface_width: config.width,
                surface_height: config.height,
                factory,
                retired_video_surfaces: VecDeque::new(),
            };
            this.recreate_backbuffer(true)?;
            this.sync_overlay_video_scale_state();
            this.wait_for_initial_composition_ready();
            log_event(
                "init",
                &[
                    ("width", Value::from(config.width as i64)),
                    ("height", Value::from(config.height as i64)),
                    ("buffer_count", Value::from(3)),
                    ("latency", Value::from(1)),
                    ("test_overlay", Value::from(config.test_overlay)),
                    ("egui_overlay", Value::from(config.egui_overlay)),
                    ("scale_filter", Value::from(config.scale_filter.perf_name())),
                ],
            );
            crate::logger::log(format!(
                "native-presenter: video scale filter selected={} ({})",
                config.scale_filter.perf_name(),
                config.scale_filter.label()
            ));
            log_event(
                "video_scale_filter_selected",
                &[
                    ("scale_filter", Value::from(config.scale_filter.perf_name())),
                    ("label", Value::from(config.scale_filter.label())),
                ],
            );
            Ok((this, active_topology, startup_window_intents))
        }
    }

    fn wait_for_initial_composition_ready(&self) {
        let wait_t0 = Instant::now();
        let commit_ok = unsafe { self._dcomp_device.WaitForCommitCompletion() }.is_ok();
        let after_commit_ms = wait_t0.elapsed().as_secs_f64() * 1000.0;
        let flush_ok = unsafe { DwmFlush() }.is_ok();
        let total_ms = wait_t0.elapsed().as_secs_f64() * 1000.0;
        crate::logger::log(format!(
            "native-presenter: initial composition ready commit_ok={commit_ok} \
             flush_ok={flush_ok} commit_ms={after_commit_ms:.2} total_ms={total_ms:.2}"
        ));
        log_event(
            "initial_composition_ready",
            &[
                ("commit_ok", Value::from(commit_ok)),
                ("flush_ok", Value::from(flush_ok)),
                ("commit_ms", Value::from(after_commit_ms)),
                ("total_ms", Value::from(total_ms)),
            ],
        );
    }

    pub(crate) fn resize(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<Vec<NativeWindowIntent>, String> {
        let width = width.max(1);
        let height = height.max(1);
        let mut window_intents = Vec::new();
        if self.width == width && self.height == height {
            return Ok(window_intents);
        }
        // T26 (Codex P2 2026-05-16): self.width/height は **inner resize がすべて成功した後** に
        // 更新する。途中で失敗すると self.width だけ更新されて size 一致になり、次回同 size
        // 呼び出しで early-return → 失敗が永久に残る。子側 (`_background`, overlays) は
        // 自身の backbuffer.is_some() で個別に retry できる (T26 子側の fix) ので、外側は
        // 子の Ok を全部見届けるまで自分の state を進めない方針にする。
        self._background
            .resize(&self.d3d_device1, &self.d3d_context, width, height)?;
        // 新しい `width`/`height` を明示的に渡す。`self.width`/`self.height` は本関数の
        // 末尾で初めて更新されるため、ここで `self.width` を読むと 1 resize 前のサイズで
        // transform を計算してしまう。
        self.update_video_visual_transform(width, height)?;
        if let Some(overlay) = self.test_overlay.as_mut() {
            overlay.resize(
                &self.d3d_device1,
                &self.d3d_context,
                &self.d3d_context1,
                width,
                height,
            )?;
        }
        if let Some(overlay) = self.egui_overlay.as_mut() {
            window_intents = overlay.resize(width, height)?;
        }
        // 全 inner resize 成功で初めて self.width/height を進める。
        self.width = width;
        self.height = height;
        self.resize_settle = VideoResizeSettleState::Waiting {
            last_geometry_change: Instant::now(),
        };
        log_event(
            "resize",
            &[
                ("width", Value::from(width as i64)),
                ("height", Value::from(height as i64)),
                ("surface_width", Value::from(self.surface_width as i64)),
                ("surface_height", Value::from(self.surface_height as i64)),
            ],
        );
        Ok(window_intents)
    }

    /// True once the latest geometry event has remained unchanged for the
    /// settle interval. The render loop uses this only to re-present the held
    /// visible frame while paused; active playback naturally consumes it on the
    /// next decoded frame.
    pub(crate) fn take_settled_resize_refresh_due(&mut self, now: Instant) -> bool {
        if !self.resize_settle.refresh_due(now) {
            return false;
        }
        // Consume the edge before performing GPU work. A failure is logged by
        // the caller and is not converted into an unbounded retry loop.
        self.resize_settle = VideoResizeSettleState::Settled;
        self.panorama_pose.is_some() || self.selected_scale_filter != VideoScaleFilter::OsDefault
    }

    pub fn set_video_compact(&mut self, compact: bool) -> Result<(), String> {
        if self.video_compact == compact {
            return Ok(());
        }
        self.video_compact = compact;
        self.update_video_visual_transform(self.width, self.height)?;
        log_event(
            "video_compact",
            &[("compact", Value::from(self.video_compact))],
        );
        Ok(())
    }

    pub fn set_video_geometry(
        &mut self,
        num: u32,
        den: u32,
        orientation: VideoOrientation,
    ) -> Result<(), String> {
        let num = num.max(1);
        let den = den.max(1);
        if self.sar_num == num && self.sar_den == den && self.orientation == orientation {
            return Ok(());
        }
        self.sar_num = num;
        self.sar_den = den;
        self.orientation = orientation;
        self.update_video_visual_transform(self.width, self.height)?;
        log_event(
            "video_geometry",
            &[
                ("sar_num", Value::from(self.sar_num as i64)),
                ("sar_den", Value::from(self.sar_den as i64)),
                (
                    "rotation_degrees",
                    Value::from(self.orientation.rotation_degrees() as i64),
                ),
                ("reflected", Value::from(self.orientation.is_reflected())),
            ],
        );
        Ok(())
    }

    pub fn present(
        &mut self,
        frame: &VideoFrame,
        sync_interval: u32,
    ) -> Result<NativePresentOutcome, String> {
        let source_width = frame.width.max(1);
        let source_height = frame.height.max(1);
        let new_sar_num = frame.sar_num.max(1);
        let new_sar_den = frame.sar_den.max(1);
        let new_orientation = frame.orientation;
        let now = Instant::now();
        let grade_active = !self.video_grade.adjustments.is_identity();
        let decision = decide_video_surface_size(VideoSurfaceSizeInput {
            filter: self.selected_scale_filter,
            panorama_active: self.panorama_pose.is_some(),
            source_width,
            source_height,
            sar_num: new_sar_num,
            sar_den: new_sar_den,
            orientation: new_orientation,
            viewport_width: self.width,
            viewport_height: self.height,
            layout: self.video_visual_layout(),
            resizing: self.resize_settle.is_resizing(now),
            current_surface_width: self.surface_width,
            current_surface_height: self.surface_height,
            current_surface_is_display_resolved: self.surface_content.is_resolved(),
        });
        let kept_current_during_resize = matches!(
            &decision,
            VideoSurfaceSizeDecision::KeepCurrentDuringResize { .. }
        );

        let result = match decision {
            VideoSurfaceSizeDecision::KeepCurrentDuringResize { .. } => {
                let current_width = self.surface_width;
                let current_height = self.surface_height;
                let current_content = self.surface_content;
                if current_content.is_resolved() {
                    let key = DisplaySurfaceRequestKey {
                        source_width,
                        source_height,
                        target_width: current_width,
                        target_height: current_height,
                        sar_num: new_sar_num,
                        sar_den: new_sar_den,
                        orientation: new_orientation,
                        panorama_active: self.panorama_pose.is_some(),
                        grade_active,
                        filter: self.selected_scale_filter,
                        smoothing_percent: self.downscale_smoothing_percent,
                        anime4k_variant: self.selected_anime4k_variant,
                    };
                    if let Err(reason) = self.prepare_display_resources(
                        source_width,
                        source_height,
                        current_width,
                        current_height,
                        new_orientation,
                        self.panorama_pose.is_some(),
                        grade_active,
                        self.selected_scale_filter,
                        self.downscale_smoothing_percent,
                        self.selected_anime4k_variant,
                    ) {
                        self.record_scale_fallback(key, reason);
                        self.present_for_surface(
                            frame,
                            sync_interval,
                            source_width,
                            source_height,
                            VideoSurfaceContent::LegacySource,
                            new_sar_num,
                            new_sar_den,
                            new_orientation,
                        )
                    } else {
                        self.present_for_surface(
                            frame,
                            sync_interval,
                            current_width,
                            current_height,
                            current_content,
                            new_sar_num,
                            new_sar_den,
                            new_orientation,
                        )
                    }
                } else {
                    self.present_for_surface(
                        frame,
                        sync_interval,
                        current_width,
                        current_height,
                        current_content,
                        new_sar_num,
                        new_sar_den,
                        new_orientation,
                    )
                }
            }
            VideoSurfaceSizeDecision::LegacySource { width, height } => {
                self.active_scale_fallback = None;
                self.present_for_surface(
                    frame,
                    sync_interval,
                    width,
                    height,
                    VideoSurfaceContent::LegacySource,
                    new_sar_num,
                    new_sar_den,
                    new_orientation,
                )
            }
            VideoSurfaceSizeDecision::FallbackToLegacy {
                width,
                height,
                reason,
            } => {
                let (target_width, target_height) = match &reason {
                    VideoScaleFallbackReason::DisplaySizeLimitExceeded {
                        requested_width,
                        requested_height,
                        ..
                    } => (*requested_width, *requested_height),
                    _ => (width, height),
                };
                let key = DisplaySurfaceRequestKey {
                    source_width,
                    source_height,
                    target_width,
                    target_height,
                    sar_num: new_sar_num,
                    sar_den: new_sar_den,
                    orientation: new_orientation,
                    panorama_active: self.panorama_pose.is_some(),
                    grade_active,
                    filter: self.selected_scale_filter,
                    smoothing_percent: self.downscale_smoothing_percent,
                    anime4k_variant: self.selected_anime4k_variant,
                };
                self.record_scale_fallback(key, reason);
                self.present_for_surface(
                    frame,
                    sync_interval,
                    width,
                    height,
                    VideoSurfaceContent::LegacySource,
                    new_sar_num,
                    new_sar_den,
                    new_orientation,
                )
            }
            VideoSurfaceSizeDecision::DisplayResolution { width, height } => {
                let key = DisplaySurfaceRequestKey {
                    source_width,
                    source_height,
                    target_width: width,
                    target_height: height,
                    sar_num: new_sar_num,
                    sar_den: new_sar_den,
                    orientation: new_orientation,
                    panorama_active: self.panorama_pose.is_some(),
                    grade_active,
                    filter: self.selected_scale_filter,
                    smoothing_percent: self.downscale_smoothing_percent,
                    anime4k_variant: self.selected_anime4k_variant,
                };
                if self
                    .active_scale_fallback
                    .as_ref()
                    .is_some_and(|fallback| fallback.key == key)
                {
                    self.present_for_surface(
                        frame,
                        sync_interval,
                        source_width,
                        source_height,
                        VideoSurfaceContent::LegacySource,
                        new_sar_num,
                        new_sar_den,
                        new_orientation,
                    )
                } else if let Err(reason) = self.prepare_display_resources(
                    source_width,
                    source_height,
                    width,
                    height,
                    new_orientation,
                    self.panorama_pose.is_some(),
                    grade_active,
                    self.selected_scale_filter,
                    self.downscale_smoothing_percent,
                    self.selected_anime4k_variant,
                ) {
                    self.record_scale_fallback(key, reason);
                    self.present_for_surface(
                        frame,
                        sync_interval,
                        source_width,
                        source_height,
                        VideoSurfaceContent::LegacySource,
                        new_sar_num,
                        new_sar_den,
                        new_orientation,
                    )
                } else {
                    let display_result = self.present_for_surface_typed(
                        frame,
                        sync_interval,
                        width,
                        height,
                        VideoSurfaceContent::resolved(self.panorama_pose.is_some()),
                        new_sar_num,
                        new_sar_den,
                        new_orientation,
                    );
                    match display_result {
                        Ok(outcome) => {
                            self.active_scale_fallback = None;
                            Ok(outcome)
                        }
                        Err(VideoSurfaceSwapError::DisplayCreation(reason)) => {
                            self.record_scale_fallback(key, reason);
                            self.present_for_surface(
                                frame,
                                sync_interval,
                                source_width,
                                source_height,
                                VideoSurfaceContent::LegacySource,
                                new_sar_num,
                                new_sar_den,
                                new_orientation,
                            )
                        }
                        Err(VideoSurfaceSwapError::Other(error)) => Err(error),
                    }
                }
            }
        };
        if result.is_ok()
            && !kept_current_during_resize
            && (self.panorama_pose.is_some()
                || self.selected_scale_filter != VideoScaleFilter::OsDefault)
        {
            self.resize_settle = VideoResizeSettleState::Settled;
        }
        self.sync_overlay_video_scale_state();
        result
    }

    /// Prepare every dimension-dependent resource for a desired scaling
    /// selection without publishing it. Typed failures are cached as an
    /// explicit legacy-path fallback for the same-unit present.
    pub(crate) fn prepare_video_scale_settings(
        &mut self,
        frame: &VideoFrame,
        identity: crate::video::VideoScaleRequestIdentity,
        filter: VideoScaleFilter,
        smoothing_percent: u32,
        anime4k_variant: Option<crate::video::anime4k_policy::VideoAnime4kVariant>,
    ) -> Result<(), String> {
        let (signature, decision_fallback) = self.video_scale_preparation_plan(
            frame,
            filter,
            smoothing_percent,
            anime4k_variant,
            Instant::now(),
        );
        self.prepared_video_scale_settings = None;

        match self.prepare_video_scale_resources(signature, decision_fallback) {
            Ok(signature) => {
                self.prepared_video_scale_settings = Some(PreparedVideoScaleSettings {
                    identity,
                    signature,
                });
                Ok(())
            }
            Err(error) => {
                self.prepared_video_surface = None;
                self.discard_candidate_fallback(signature);
                self.sync_overlay_video_scale_state();
                Err(error)
            }
        }
    }

    fn prepare_video_scale_resources(
        &mut self,
        mut signature: VideoScalePreparationSignature,
        decision_fallback: Option<VideoScaleFallbackReason>,
    ) -> Result<VideoScalePreparationSignature, String> {
        let smoothing_percent = signature.smoothing_percent;
        if let Some(reason) = decision_fallback {
            let (requested_width, requested_height) = match &reason {
                VideoScaleFallbackReason::DisplaySizeLimitExceeded {
                    requested_width,
                    requested_height,
                    ..
                } => (*requested_width, *requested_height),
                _ => (signature.target_width, signature.target_height),
            };
            self.record_scale_fallback(
                Self::video_scale_request_key(signature, requested_width, requested_height),
                reason,
            );
        } else if signature.target_content.is_resolved() {
            let key = Self::video_scale_request_key(
                signature,
                signature.target_width,
                signature.target_height,
            );
            if let Err(reason) = self.prepare_display_resources(
                signature.source_width,
                signature.source_height,
                signature.target_width,
                signature.target_height,
                signature.orientation,
                signature.panorama_active,
                signature.grade_active,
                signature.filter,
                smoothing_percent,
                signature.anime4k_variant,
            ) {
                self.record_scale_fallback(key.clone(), reason);
                signature.target_width = signature.source_width;
                signature.target_height = signature.source_height;
                signature.target_content = VideoSurfaceContent::LegacySource;
            }
        }

        if signature.target_content.is_resolved() {
            let key = Self::video_scale_request_key(
                signature,
                signature.target_width,
                signature.target_height,
            );
            if let Err(error) = self.prepare_scale_target_surface(
                signature.target_width,
                signature.target_height,
                signature.target_content,
            ) {
                match error {
                    VideoSurfaceSwapError::DisplayCreation(reason) => {
                        self.record_scale_fallback(key, reason);
                        signature.target_width = signature.source_width;
                        signature.target_height = signature.source_height;
                        signature.target_content = VideoSurfaceContent::LegacySource;
                    }
                    VideoSurfaceSwapError::Other(error) => return Err(error),
                }
            } else if self
                .active_scale_fallback
                .as_ref()
                .is_some_and(|fallback| fallback.key == key)
            {
                // An explicit preparation is also the retry boundary for a
                // transient allocation failure. Do not let the old typed
                // fallback mask resources that are now fully ready.
                self.active_scale_fallback = None;
            }
        }

        if signature.target_content == VideoSurfaceContent::LegacySource {
            self.prepare_scale_target_surface(
                signature.target_width,
                signature.target_height,
                signature.target_content,
            )
            .map_err(VideoSurfaceSwapError::into_string)?;
        }

        Ok(signature)
    }

    fn video_scale_preparation_plan(
        &self,
        frame: &VideoFrame,
        filter: VideoScaleFilter,
        smoothing_percent: u32,
        anime4k_variant: Option<crate::video::anime4k_policy::VideoAnime4kVariant>,
        now: Instant,
    ) -> (
        VideoScalePreparationSignature,
        Option<VideoScaleFallbackReason>,
    ) {
        let source_width = frame.width.max(1);
        let source_height = frame.height.max(1);
        let sar_num = frame.sar_num.max(1);
        let sar_den = frame.sar_den.max(1);
        let decision = decide_video_surface_size(VideoSurfaceSizeInput {
            filter,
            panorama_active: self.panorama_pose.is_some(),
            source_width,
            source_height,
            sar_num,
            sar_den,
            orientation: frame.orientation,
            viewport_width: self.width,
            viewport_height: self.height,
            layout: self.video_visual_layout(),
            resizing: self.resize_settle.is_resizing(now),
            current_surface_width: self.surface_width,
            current_surface_height: self.surface_height,
            current_surface_is_display_resolved: self.surface_content.is_resolved(),
        });
        let (target_width, target_height, target_content, fallback) = match decision {
            VideoSurfaceSizeDecision::DisplayResolution { width, height } => (
                width,
                height,
                VideoSurfaceContent::resolved(self.panorama_pose.is_some()),
                None,
            ),
            VideoSurfaceSizeDecision::FallbackToLegacy {
                width,
                height,
                reason,
            } => (
                width,
                height,
                VideoSurfaceContent::LegacySource,
                Some(reason),
            ),
            VideoSurfaceSizeDecision::LegacySource { width, height } => {
                (width, height, VideoSurfaceContent::LegacySource, None)
            }
            VideoSurfaceSizeDecision::KeepCurrentDuringResize { width, height } => {
                (width, height, self.surface_content, None)
            }
        };
        (
            VideoScalePreparationSignature {
                source_width,
                source_height,
                sar_num,
                sar_den,
                orientation: frame.orientation,
                target_width,
                target_height,
                target_content,
                panorama_active: self.panorama_pose.is_some(),
                grade_active: !self.video_grade.adjustments.is_identity(),
                filter,
                smoothing_percent: crate::settings::sanitize_downscale_smoothing_percent(
                    smoothing_percent,
                ),
                anime4k_variant,
            },
            fallback,
        )
    }

    fn current_video_scale_preparation_signature(
        &self,
        frame: &VideoFrame,
        filter: VideoScaleFilter,
        smoothing_percent: u32,
        anime4k_variant: Option<crate::video::anime4k_policy::VideoAnime4kVariant>,
    ) -> VideoScalePreparationSignature {
        let (mut signature, _) = self.video_scale_preparation_plan(
            frame,
            filter,
            smoothing_percent,
            anime4k_variant,
            Instant::now(),
        );
        if signature.target_content.is_resolved() {
            let key = Self::video_scale_request_key(
                signature,
                signature.target_width,
                signature.target_height,
            );
            if self
                .active_scale_fallback
                .as_ref()
                .is_some_and(|fallback| fallback.key == key)
            {
                signature.target_width = signature.source_width;
                signature.target_height = signature.source_height;
                signature.target_content = VideoSurfaceContent::LegacySource;
            }
        }
        signature
    }

    fn video_scale_request_key(
        signature: VideoScalePreparationSignature,
        target_width: u32,
        target_height: u32,
    ) -> DisplaySurfaceRequestKey {
        DisplaySurfaceRequestKey {
            source_width: signature.source_width,
            source_height: signature.source_height,
            target_width,
            target_height,
            sar_num: signature.sar_num,
            sar_den: signature.sar_den,
            orientation: signature.orientation,
            panorama_active: signature.panorama_active,
            grade_active: signature.grade_active,
            filter: signature.filter,
            smoothing_percent: signature.smoothing_percent,
            anime4k_variant: signature.anime4k_variant,
        }
    }

    pub(crate) fn present_prepared_video_scale_settings(
        &mut self,
        frame: &VideoFrame,
        sync_interval: u32,
        identity: crate::video::VideoScaleRequestIdentity,
        filter: VideoScaleFilter,
        smoothing_percent: u32,
        anime4k_variant: Option<crate::video::anime4k_policy::VideoAnime4kVariant>,
    ) -> Result<NativePresentOutcome, VideoScalePreparedPresentError> {
        let signature = self.current_video_scale_preparation_signature(
            frame,
            filter,
            smoothing_percent,
            anime4k_variant,
        );
        if let Err(reason) = validate_prepared_video_scale_settings(
            self.prepared_video_scale_settings,
            identity,
            signature,
        ) {
            self.discard_prepared_video_scale_settings();
            return Err(VideoScalePreparedPresentError::Rejected(reason));
        }
        self.prepared_video_scale_settings = None;
        let previous = (
            self.selected_scale_filter,
            self.downscale_smoothing_percent,
            self.selected_anime4k_variant,
        );
        self.selected_scale_filter = filter;
        self.downscale_smoothing_percent =
            crate::settings::sanitize_downscale_smoothing_percent(smoothing_percent);
        self.selected_anime4k_variant = anime4k_variant;
        match self.present(frame, sync_interval) {
            Ok(outcome) => {
                self.record_video_scale_settings_selected();
                Ok(outcome)
            }
            Err(error) => {
                self.selected_scale_filter = previous.0;
                self.downscale_smoothing_percent = previous.1;
                self.selected_anime4k_variant = previous.2;
                self.discard_candidate_fallback(signature);
                self.sync_overlay_video_scale_state();
                Err(VideoScalePreparedPresentError::Present(error))
            }
        }
    }

    pub(crate) fn discard_prepared_video_scale_settings(&mut self) {
        if let Some(prepared) = self.prepared_video_scale_settings.take() {
            self.discard_candidate_fallback(prepared.signature);
        }
        self.prepared_video_surface = None;
        self.sync_overlay_video_scale_state();
    }

    fn discard_candidate_fallback(&mut self, signature: VideoScalePreparationSignature) {
        if self.active_scale_fallback.as_ref().is_some_and(|fallback| {
            fallback.key.filter == signature.filter
                && fallback.key.smoothing_percent == signature.smoothing_percent
                && fallback.key.anime4k_variant == signature.anime4k_variant
                && (self.selected_scale_filter != signature.filter
                    || self.downscale_smoothing_percent != signature.smoothing_percent
                    || self.selected_anime4k_variant != signature.anime4k_variant)
        }) {
            self.active_scale_fallback = None;
        }
    }

    fn record_video_scale_settings_selected(&mut self) {
        let filter = self.selected_scale_filter;
        let anime4k_variant = self.selected_anime4k_variant;
        crate::logger::log(format!(
            "native-presenter: video scale filter selected={} ({}) smoothing_percent={} anime4k_variant={}",
            filter.perf_name(),
            filter.label(),
            self.downscale_smoothing_percent,
            anime4k_variant
                .map(|variant| variant.label())
                .unwrap_or("none")
        ));
        log_event(
            "video_scale_filter_selected",
            &[
                ("scale_filter", Value::from(filter.perf_name())),
                ("label", Value::from(filter.label())),
                (
                    "downscale_smoothing_percent",
                    Value::from(self.downscale_smoothing_percent as i64),
                ),
                (
                    "anime4k_variant",
                    Value::from(
                        anime4k_variant
                            .map(|variant| variant.label())
                            .unwrap_or("none"),
                    ),
                ),
            ],
        );
        self.sync_overlay_video_scale_state();
    }

    pub(crate) fn video_scale_fallback_notice(&self) -> Option<VideoScaleFallbackNotice> {
        if let Some(fallback) = self.active_scale_fallback.as_ref().filter(|fallback| {
            fallback.key.filter == self.selected_scale_filter
                && fallback.key.smoothing_percent == self.downscale_smoothing_percent
                && fallback.key.anime4k_variant == self.selected_anime4k_variant
        }) {
            return Some(match &fallback.reason {
                VideoScaleFallbackReason::DisplaySizeLimitExceeded {
                    requested_width,
                    requested_height,
                    max_dimension,
                    max_pixels,
                } => VideoScaleFallbackNotice::DisplaySizeLimitExceeded {
                    requested_width: *requested_width,
                    requested_height: *requested_height,
                    max_dimension: *max_dimension,
                    max_pixels: *max_pixels,
                },
                _ => VideoScaleFallbackNotice::Other,
            });
        }
        if self.selected_scale_filter != VideoScaleFilter::Anime
            || self.selected_anime4k_variant.is_some()
        {
            return None;
        }
        Some(VideoScaleFallbackNotice::Anime4kStandard(
            match self.anime4k_status {
                NativeVideoAnime4kStatus::Waiting => VideoAnime4kFallbackNotice::Waiting,
                NativeVideoAnime4kStatus::Measuring { .. } => VideoAnime4kFallbackNotice::Measuring,
                NativeVideoAnime4kStatus::Fallback(reason) => {
                    VideoAnime4kFallbackNotice::Fallback(reason)
                }
                NativeVideoAnime4kStatus::Failed => VideoAnime4kFallbackNotice::Failed,
                NativeVideoAnime4kStatus::Active { .. } => return None,
            },
        ))
    }
    fn sync_overlay_video_scale_state(&mut self) {
        let outside_note = if self.panorama_pose.is_some() {
            Some("360 投影中のため一時停止しています".to_owned())
        } else {
            self.active_scale_fallback.as_ref().and_then(|fallback| {
                if fallback.key.filter != self.selected_scale_filter
                    || fallback.key.smoothing_percent != self.downscale_smoothing_percent
                    || fallback.key.anime4k_variant != self.selected_anime4k_variant
                {
                    return None;
                }
                let VideoScaleFallbackReason::DisplaySizeLimitExceeded {
                    max_dimension,
                    max_pixels,
                    ..
                } = &fallback.reason
                else {
                    return None;
                };
                Some(crate::ui_helpers::processing_size_outside_note_for(
                    "動画",
                    &format!(
                        "長辺 {}px 以下・総画素数 {}px 以下",
                        max_dimension, max_pixels
                    ),
                ))
            })
        };
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_video_scale_state(
                self.selected_scale_filter,
                self.downscale_smoothing_percent,
                outside_note,
            );
            overlay.set_video_anime4k_state(self.anime4k_budget, self.anime4k_status.clone());
        }
    }

    pub fn set_overlay_video_anime4k_state(
        &mut self,
        budget: crate::video::anime4k_policy::VideoAnime4kBudgetPreset,
        status: NativeVideoAnime4kStatus,
    ) {
        self.anime4k_budget = budget;
        self.anime4k_status = status;
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_video_anime4k_state(budget, self.anime4k_status.clone());
        }
    }

    pub fn reset_anime4k_for_source_change(&mut self) {
        if self.selected_scale_filter != VideoScaleFilter::Anime {
            return;
        }
        self.selected_anime4k_variant = None;
        self.anime4k_status = NativeVideoAnime4kStatus::Waiting;
        self.prepared_video_scale_settings = None;
        self.sync_overlay_video_scale_state();
    }

    #[allow(clippy::too_many_arguments)]
    fn present_for_surface(
        &mut self,
        frame: &VideoFrame,
        sync_interval: u32,
        width: u32,
        height: u32,
        content: VideoSurfaceContent,
        new_sar_num: u32,
        new_sar_den: u32,
        new_orientation: VideoOrientation,
    ) -> Result<NativePresentOutcome, String> {
        self.present_for_surface_typed(
            frame,
            sync_interval,
            width,
            height,
            content,
            new_sar_num,
            new_sar_den,
            new_orientation,
        )
        .map_err(VideoSurfaceSwapError::into_string)
    }

    #[allow(clippy::too_many_arguments)]
    fn present_for_surface_typed(
        &mut self,
        frame: &VideoFrame,
        sync_interval: u32,
        width: u32,
        height: u32,
        content: VideoSurfaceContent,
        new_sar_num: u32,
        new_sar_den: u32,
        new_orientation: VideoOrientation,
    ) -> Result<NativePresentOutcome, VideoSurfaceSwapError> {
        let surface_changed = self.surface_width != width
            || self.surface_height != height
            || self.surface_content != content;
        if surface_changed {
            self.present_with_surface_swap(
                frame,
                sync_interval,
                width,
                height,
                content,
                new_sar_num,
                new_sar_den,
                new_orientation,
            )
        } else {
            self.present_reusing_surface(
                frame,
                sync_interval,
                new_sar_num,
                new_sar_den,
                new_orientation,
            )
            .map_err(VideoSurfaceSwapError::Other)
        }
    }

    fn resample_mode(
        &self,
        source_width: u32,
        source_height: u32,
        target_width: u32,
        target_height: u32,
        orientation: VideoOrientation,
    ) -> Option<VideoResampleMode> {
        let (source_axis_width, source_axis_height) = if orientation.swaps_axes() {
            (source_height, source_width)
        } else {
            (source_width, source_height)
        };
        select_video_resample_mode(
            self.selected_scale_filter,
            source_axis_width,
            source_axis_height,
            target_width,
            target_height,
            self.downscale_smoothing_percent,
            self.selected_anime4k_variant,
        )
    }

    fn prepare_display_resources(
        &mut self,
        source_width: u32,
        source_height: u32,
        target_width: u32,
        target_height: u32,
        orientation: VideoOrientation,
        panorama_active: bool,
        grade_active: bool,
        filter: VideoScaleFilter,
        smoothing_percent: u32,
        anime4k_variant: Option<crate::video::anime4k_policy::VideoAnime4kVariant>,
    ) -> Result<(), VideoScaleFallbackReason> {
        if panorama_active {
            let Some(pipeline) = self.panorama_pipeline.as_mut() else {
                return Err(VideoScaleFallbackReason::PanoramaPipelineUnavailable {
                    error: self
                        .panorama_pipeline_error
                        .clone()
                        .unwrap_or_else(|| "pipeline was not created".to_string()),
                });
            };
            let (oriented_width, oriented_height) = if orientation.swaps_axes() {
                (source_height, source_width)
            } else {
                (source_width, source_height)
            };
            pipeline
                .prepare(&self.d3d_device1, source_width, source_height, orientation)
                .map_err(|error| {
                    VideoScaleFallbackReason::PanoramaOrientationIntermediateCreationFailed {
                        width: oriented_width,
                        height: oriented_height,
                        error,
                    }
                })?;
            if grade_active {
                self.grade_pipeline
                    .prepare_intermediate(&self.d3d_device1, source_width, source_height)
                    .map_err(
                        |error| VideoScaleFallbackReason::GradeIntermediateCreationFailed {
                            width: source_width,
                            height: source_height,
                            error,
                        },
                    )?;
            }
            return Ok(());
        }
        let (source_axis_width, source_axis_height) = if orientation.swaps_axes() {
            (source_height, source_width)
        } else {
            (source_width, source_height)
        };
        let mode = select_video_resample_mode(
            filter,
            source_axis_width,
            source_axis_height,
            target_width,
            target_height,
            smoothing_percent,
            anime4k_variant,
        )
        .expect("display-resolution surface always has a resample owner");
        let Some(pipeline) = self.resample_pipeline.as_mut() else {
            return Err(VideoScaleFallbackReason::ResamplePipelineUnavailable {
                error: self
                    .resample_pipeline_error
                    .clone()
                    .unwrap_or_else(|| "pipeline was not created".to_string()),
            });
        };
        let (intermediate_width, intermediate_height) =
            VideoResamplePipeline::intermediate_dimensions(
                source_width,
                source_height,
                target_width,
                orientation,
            );
        pipeline
            .prepare(
                &self.d3d_device1,
                source_width,
                source_height,
                target_width,
                orientation,
                mode,
            )
            .map_err(|error| match error {
                VideoResamplePrepareError::IntermediateCreation(error) => {
                    VideoScaleFallbackReason::ResampleIntermediateCreationFailed {
                        width: intermediate_width,
                        height: intermediate_height,
                        error,
                    }
                }
                VideoResamplePrepareError::Anime4kPipelineUnavailable { variant, error } => {
                    VideoScaleFallbackReason::Anime4kPipelineUnavailable {
                        variant: variant.label(),
                        error,
                    }
                }
                VideoResamplePrepareError::Anime4kIntermediateCreationFailed {
                    variant,
                    pass_index,
                    width,
                    height,
                    error,
                } => VideoScaleFallbackReason::Anime4kIntermediateCreationFailed {
                    variant: variant.label(),
                    pass_index,
                    width,
                    height,
                    error,
                },
            })?;
        if grade_active {
            self.grade_pipeline
                .prepare_intermediate(&self.d3d_device1, source_width, source_height)
                .map_err(
                    |error| VideoScaleFallbackReason::GradeIntermediateCreationFailed {
                        width: source_width,
                        height: source_height,
                        error,
                    },
                )?;
        }
        Ok(())
    }

    fn record_scale_fallback(
        &mut self,
        key: DisplaySurfaceRequestKey,
        reason: VideoScaleFallbackReason,
    ) {
        if self
            .active_scale_fallback
            .as_ref()
            .is_some_and(|active| active.key == key && active.reason == reason)
        {
            return;
        }
        self.scale_fallback_count = self.scale_fallback_count.saturating_add(1);
        crate::logger::log(format!(
            "native-presenter: video scale fallback filter={} reason={} count={} {}",
            key.filter.perf_name(),
            reason.code(),
            self.scale_fallback_count,
            reason.detail()
        ));
        log_event(
            "video_scale_fallback",
            &[
                ("scale_filter", Value::from(key.filter.perf_name())),
                ("reason", Value::from(reason.code())),
                ("reason_detail", Value::from(reason.detail())),
                (
                    "fallback_count",
                    Value::from(self.scale_fallback_count as i64),
                ),
                ("source_width", Value::from(key.source_width as i64)),
                ("source_height", Value::from(key.source_height as i64)),
                ("target_width", Value::from(key.target_width as i64)),
                ("target_height", Value::from(key.target_height as i64)),
            ],
        );
        self.active_scale_fallback = Some(ActiveVideoScaleFallback { key, reason });
    }

    /// 解像度不変時の present。既存の `swap_chain` / `backbuffer` をそのまま使う。
    /// SAR / orientation だけ変わった場合は transform を再計算する
    /// (swap chain content は正しいので中間状態は生じない)。
    fn present_reusing_surface(
        &mut self,
        frame: &VideoFrame,
        sync_interval: u32,
        new_sar_num: u32,
        new_sar_den: u32,
        new_orientation: VideoOrientation,
    ) -> Result<NativePresentOutcome, String> {
        let wait_t0 = Instant::now();
        let wait_result = unsafe { WaitForSingleObject(self.waitable, 100) };
        let wait_ms = wait_t0.elapsed().as_secs_f64() * 1000.0;
        let timed_out = wait_result == WAIT_TIMEOUT;

        let geometry_changed = self.sar_num != new_sar_num
            || self.sar_den != new_sar_den
            || self.orientation != new_orientation;
        if geometry_changed {
            // 表示メタデータだけ変わった (= 同一解像度の動画へ切替)。swap chain の
            // サイズは正しいので transform を作り直すだけでよい。
            self.sar_num = new_sar_num;
            self.sar_den = new_sar_den;
            self.orientation = new_orientation;
            self.update_video_visual_transform(self.width, self.height)?;
        }

        let copy_t0 = Instant::now();
        let backbuffer = self
            .backbuffer
            .clone()
            .ok_or_else(|| "native presenter backbuffer is not initialized".to_string())?;
        let copy = self.copy_frame_into_backbuffer(
            frame,
            &backbuffer,
            self.surface_content,
            self.surface_width,
            self.surface_height,
        )?;
        let copy_ms = copy_t0.elapsed().as_secs_f64() * 1000.0;

        let present_t0 = Instant::now();
        let hr = {
            let _operation = self
                .health
                .begin_render_operation(NativeRenderOperation::Present, self.window_epoch);
            unsafe { self.swap_chain.Present(sync_interval, Default::default()) }
        };
        if hr.is_err() {
            return Err(format!("IDXGISwapChain::Present: {hr:?}"));
        }
        let present_call_ms = present_t0.elapsed().as_secs_f64() * 1000.0;

        Ok(NativePresentOutcome {
            path: copy.path,
            shared_handle: copy.shared_handle,
            shared_cache_hit: copy.shared_cache_hit,
            shared_texture_gen: copy.shared_texture_gen,
            fence_value: copy.fence_value,
            copy_fence_value: copy.copy_fence_value,
            surface_width: self.surface_width,
            surface_height: self.surface_height,
            geometry_changed,
            surface_swapped: false,
            commit_sync_ms: 0.0,
            wait_ms,
            wait_timed_out: timed_out,
            fence_wait_ms: copy.fence_wait_ms,
            open_shared_ms: copy.open_shared_ms,
            keyed_mutex_ms: copy.keyed_mutex_ms,
            keyed_mutex_cast_ms: copy.keyed_mutex_cast_ms,
            keyed_mutex_acquire_ms: copy.keyed_mutex_acquire_ms,
            copy_call_ms: copy.copy_call_ms,
            copy_ms,
            present_waitable_ms: wait_ms,
            present_call_ms,
            present_ms: present_call_ms,
        })
    }

    /// 解像度変更時の present。**新しい video swap chain を別途生成し、最初の正しい
    /// フレームを `Present` 済みにしてから、`SetContent` + `SetTransform2` を 1 回の
    /// `Commit` で原子的に差し替える**。旧 swap chain は Commit まで visual に
    /// 繋がったまま (= 正しい映像のまま) なので、黒や「左上にずれた中間フレーム」が
    /// 一切表示されない。`ResizeBuffers` (旧 content を破棄する) は使わない。
    ///
    /// 旧 swap chain は `WaitForCommitCompletion` 後も即 drop せず `retired_video_surfaces`
    /// に数世代分残す (DComp/DWM がまだ旧 content を参照しうるため。Codex 助言)。
    fn prepare_scale_target_surface(
        &mut self,
        width: u32,
        height: u32,
        content: VideoSurfaceContent,
    ) -> Result<(), VideoSurfaceSwapError> {
        if self.surface_width == width
            && self.surface_height == height
            && self.surface_content == content
        {
            self.prepared_video_surface = None;
            return Ok(());
        }
        if self
            .prepared_video_surface
            .as_ref()
            .is_some_and(|surface| surface.matches(width, height, content))
        {
            return Ok(());
        }
        self.prepared_video_surface =
            Some(self.create_prepared_video_surface(width, height, content)?);
        Ok(())
    }

    fn create_prepared_video_surface(
        &self,
        width: u32,
        height: u32,
        content: VideoSurfaceContent,
    ) -> Result<PreparedVideoSurface, VideoSurfaceSwapError> {
        let (swap_chain, waitable) =
            self.create_video_swap_chain(width, height)
                .map_err(|error| {
                    if content.is_resolved() {
                        VideoSurfaceSwapError::DisplayCreation(
                            VideoScaleFallbackReason::DisplaySwapChainCreationFailed {
                                width,
                                height,
                                error,
                            },
                        )
                    } else {
                        VideoSurfaceSwapError::Other(error)
                    }
                })?;
        let mut surface = PreparedVideoSurface {
            width,
            height,
            content,
            swap_chain: Some(swap_chain),
            waitable,
            backbuffer: None,
        };
        let backbuffer = self
            .create_swap_chain_backbuffer(
                surface
                    .swap_chain
                    .as_ref()
                    .expect("prepared video surface swap chain"),
            )
            .map_err(|error| {
                if content.is_resolved() {
                    VideoSurfaceSwapError::DisplayCreation(
                        VideoScaleFallbackReason::DisplayBackbufferCreationFailed {
                            width,
                            height,
                            error,
                        },
                    )
                } else {
                    VideoSurfaceSwapError::Other(error)
                }
            })?;
        surface.backbuffer = Some(backbuffer);
        Ok(surface)
    }

    fn take_or_create_video_surface(
        &mut self,
        width: u32,
        height: u32,
        content: VideoSurfaceContent,
    ) -> Result<PreparedVideoSurface, VideoSurfaceSwapError> {
        if self
            .prepared_video_surface
            .as_ref()
            .is_some_and(|surface| surface.matches(width, height, content))
        {
            return Ok(self
                .prepared_video_surface
                .take()
                .expect("matching prepared video surface"));
        }
        self.prepared_video_surface = None;
        self.create_prepared_video_surface(width, height, content)
    }

    fn present_with_surface_swap(
        &mut self,
        frame: &VideoFrame,
        sync_interval: u32,
        new_w: u32,
        new_h: u32,
        new_content: VideoSurfaceContent,
        new_sar_num: u32,
        new_sar_den: u32,
        new_orientation: VideoOrientation,
    ) -> Result<NativePresentOutcome, VideoSurfaceSwapError> {
        let geom_t0 = Instant::now();
        // 1. A filter switch consumes the unattached surface created by its
        // prepare command. Other lifecycle swaps use the same constructor here.
        let mut new_surface = self.take_or_create_video_surface(new_w, new_h, new_content)?;
        let new_swap_chain = new_surface
            .swap_chain
            .take()
            .expect("prepared video surface swap chain");
        let new_backbuffer = new_surface
            .backbuffer
            .take()
            .expect("prepared video surface backbuffer");

        struct WaitableGuard(HANDLE);
        impl Drop for WaitableGuard {
            fn drop(&mut self) {
                if !self.0.is_invalid() {
                    unsafe {
                        let _ = CloseHandle(self.0);
                    }
                }
            }
        }
        let mut new_waitable_guard = WaitableGuard(std::mem::replace(
            &mut new_surface.waitable,
            HANDLE::default(),
        ));
        // 2. 最初のフレームを新 backbuffer へコピーする。
        let copy_t0 = Instant::now();
        let copy =
            self.copy_frame_into_backbuffer(frame, &new_backbuffer, new_content, new_w, new_h)?;
        let copy_ms = copy_t0.elapsed().as_secs_f64() * 1000.0;

        // 3. 新 swap chain を Present (= 新 swap chain は「正しいフレーム投入済み」状態)。
        let present_t0 = Instant::now();
        {
            let _operation = self
                .health
                .begin_render_operation(NativeRenderOperation::Present, self.window_epoch);
            unsafe { new_swap_chain.Present(sync_interval, Default::default()) }
                .ok()
                .map_err(|e| format!("IDXGISwapChain::Present (new surface): {e:?}"))?;
        }
        let present_call_ms = present_t0.elapsed().as_secs_f64() * 1000.0;

        // 4. content + transform を 1 回の Commit で原子的に差し替える。旧 swap chain は
        //    この Commit が反映されるまで visual に繋がったまま (= 正しい映像のまま)。
        //
        //    ⚠️ transform はローカル値 (new_*) から計算し、`self.*` はまだ touch しない。
        //    `SetContent` / `SetTransform2` / `Commit` のいずれかが失敗して `?` で早期
        //    return しても、presenter の状態 (`surface_width/height`, `sar_num/den`,
        //    `swap_chain`, `backbuffer`) は旧 surface のまま完全に一貫している。さもないと
        //    「`surface_*` だけ新サイズに進み swap_chain は旧のまま」という不整合に陥り、
        //    次回以降の `present` が `present_reusing_surface` 経路へ誤って入って固着する
        //    (Codex P2)。`self.*` の更新は Commit 成功後 (手順 6) に一括で行う。
        let (transform_sar_num, transform_sar_den, transform_orientation) =
            transform_geometry_for_surface(new_content, new_sar_num, new_sar_den, new_orientation);
        let (m11, m12, m21, m22, offset_x, offset_y) = compute_video_visual_transform_for_surface(
            new_w,
            new_h,
            self.width,
            self.height,
            transform_sar_num,
            transform_sar_den,
            transform_orientation,
            self.video_visual_layout(),
            new_content == VideoSurfaceContent::PanoramaResolved,
        );
        let transform = Matrix3x2 {
            M11: m11,
            M12: m12,
            M21: m21,
            M22: m22,
            M31: offset_x,
            M32: offset_y,
        };
        let commit_operation = self
            .health
            .begin_render_operation(NativeRenderOperation::DCompCommit, self.window_epoch);
        unsafe {
            self._video_visual
                .SetContent(&new_swap_chain)
                .map_err(|e| format!("IDCompositionVisual::SetContent (surface swap): {e:?}"))?;
            self._video_visual
                .SetTransform2(&transform)
                .map_err(|e| format!("IDCompositionVisual::SetTransform2 (surface swap): {e:?}"))?;
            self._dcomp_device
                .Commit()
                .map_err(|e| format!("IDCompositionDevice::Commit (surface swap): {e:?}"))?;
        }

        // 5. Commit が DWM の compositor tick まで反映され切るまで待つ。
        let commit_sync_ms = self.wait_for_video_transform_commit();
        drop(commit_operation);

        // 6. Commit 成功。ここで初めて `self.*` を新 surface へ確定する (一括更新)。
        self.sar_num = new_sar_num;
        self.sar_den = new_sar_den;
        self.orientation = new_orientation;
        self.surface_width = new_w;
        self.surface_height = new_h;
        self.surface_content = new_content;
        self.surface_swap_count = self.surface_swap_count.saturating_add(1);
        let old_swap_chain = std::mem::replace(&mut self.swap_chain, new_swap_chain);
        // guard を disarm して waitable の所有権を self.waitable へ移す (= guard の Drop は
        // 以後 no-op になり、二重 CloseHandle を防ぐ)。
        let taken_waitable = std::mem::replace(&mut new_waitable_guard.0, HANDLE::default());
        let old_waitable = std::mem::replace(&mut self.waitable, taken_waitable);
        let old_backbuffer = self.backbuffer.replace(new_backbuffer);
        self.retired_video_surfaces.push_back(RetiredVideoSurface {
            _swap_chain: old_swap_chain,
            waitable: old_waitable,
            _backbuffer: old_backbuffer,
        });
        while self.retired_video_surfaces.len() > RETIRED_VIDEO_SURFACE_DEPTH {
            self.retired_video_surfaces.pop_front();
        }

        let geom_ms = geom_t0.elapsed().as_secs_f64() * 1000.0;
        crate::logger::log(format!(
            "native-presenter: video surface swapped to {new_w}x{new_h} content={new_content:?} \
             sar={}/{} orientation={:?} count={} geom_ms={geom_ms:.2} commit_sync_ms={commit_sync_ms:.2}",
            self.sar_num, self.sar_den, self.orientation, self.surface_swap_count
        ));
        log_event(
            "video_surface_swap",
            &[
                ("surface_width", Value::from(new_w as i64)),
                ("surface_height", Value::from(new_h as i64)),
                (
                    "surface_content",
                    Value::from(match new_content {
                        VideoSurfaceContent::LegacySource => "legacy_source",
                        VideoSurfaceContent::DisplayResolved => "display_resolved",
                        VideoSurfaceContent::PanoramaResolved => "panorama_resolved",
                    }),
                ),
                (
                    "scale_filter",
                    Value::from(self.selected_scale_filter.perf_name()),
                ),
                (
                    "surface_swap_count",
                    Value::from(self.surface_swap_count as i64),
                ),
                ("sar_num", Value::from(self.sar_num as i64)),
                ("sar_den", Value::from(self.sar_den as i64)),
                (
                    "rotation_degrees",
                    Value::from(self.orientation.rotation_degrees() as i64),
                ),
                ("reflected", Value::from(self.orientation.is_reflected())),
                ("geom_ms", Value::from(geom_ms)),
                ("commit_sync_ms", Value::from(commit_sync_ms)),
                (
                    "retired_len",
                    Value::from(self.retired_video_surfaces.len() as i64),
                ),
            ],
        );
        let (grade_intermediate_bytes, resample_intermediate_bytes) =
            self.intermediate_vram_bytes();
        crate::gpu_info::emit_vram_trace(
            "video_surface_swap",
            "after_video_swap_chain_replace",
            &[
                ("surface_width", Value::from(new_w as i64)),
                ("surface_height", Value::from(new_h as i64)),
                (
                    "retired_video_surfaces_len",
                    Value::from(self.retired_video_surfaces.len() as i64),
                ),
                (
                    "shared_texture_cache_len",
                    Value::from(self.shared_texture_cache.len() as i64),
                ),
                (
                    "grade_intermediate_bytes",
                    Value::from(grade_intermediate_bytes),
                ),
                (
                    "resample_intermediate_bytes",
                    Value::from(resample_intermediate_bytes),
                ),
            ],
        );

        Ok(NativePresentOutcome {
            path: copy.path,
            shared_handle: copy.shared_handle,
            shared_cache_hit: copy.shared_cache_hit,
            shared_texture_gen: copy.shared_texture_gen,
            fence_value: copy.fence_value,
            copy_fence_value: copy.copy_fence_value,
            surface_width: self.surface_width,
            surface_height: self.surface_height,
            geometry_changed: true,
            surface_swapped: true,
            commit_sync_ms,
            wait_ms: 0.0,
            wait_timed_out: false,
            fence_wait_ms: copy.fence_wait_ms,
            open_shared_ms: copy.open_shared_ms,
            keyed_mutex_ms: copy.keyed_mutex_ms,
            keyed_mutex_cast_ms: copy.keyed_mutex_cast_ms,
            keyed_mutex_acquire_ms: copy.keyed_mutex_acquire_ms,
            copy_call_ms: copy.copy_call_ms,
            copy_ms,
            present_waitable_ms: 0.0,
            present_call_ms,
            present_ms: present_call_ms,
        })
    }

    /// 新しい video swap chain を生成する (`new()` の生成ロジックと同一)。
    /// 戻り値は `(swap_chain, frame-latency waitable)`。
    fn create_video_swap_chain(
        &self,
        width: u32,
        height: u32,
    ) -> Result<(IDXGISwapChain1, HANDLE), String> {
        let width = width.max(1);
        let height = height.max(1);
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 3,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32,
        };
        unsafe {
            let swap_chain = self
                .factory
                .CreateSwapChainForComposition(&self.d3d_device1, &desc, None::<&IDXGIOutput>)
                .map_err(|e| format!("CreateSwapChainForComposition video: {e:?}"))?;
            let swap_chain2: IDXGISwapChain2 = swap_chain
                .cast()
                .map_err(|e| format!("cast IDXGISwapChain2 video: {e:?}"))?;
            swap_chain2
                .SetMaximumFrameLatency(1)
                .map_err(|e| format!("SetMaximumFrameLatency video: {e:?}"))?;
            let waitable = swap_chain2.GetFrameLatencyWaitableObject();
            Ok((swap_chain, waitable))
        }
    }

    /// 指定 swap chain の backbuffer を取得して黒クリアして返す (`Present` はしない)。
    fn create_swap_chain_backbuffer(
        &self,
        swap_chain: &IDXGISwapChain1,
    ) -> Result<ID3D11Texture2D, String> {
        let backbuffer: ID3D11Texture2D = unsafe {
            swap_chain
                .GetBuffer(0)
                .map_err(|e| format!("IDXGISwapChain::GetBuffer (new surface): {e:?}"))?
        };
        let mut backbuffer_view = None;
        unsafe {
            self.d3d_device1
                .CreateRenderTargetView(&backbuffer, None, Some(&mut backbuffer_view))
                .map_err(|e| format!("CreateRenderTargetView (new surface): {e:?}"))?;
        }
        let backbuffer_view: ID3D11RenderTargetView = backbuffer_view
            .ok_or_else(|| "CreateRenderTargetView returned null (new surface)".to_string())?;
        unsafe {
            self.d3d_context
                .ClearRenderTargetView(&backbuffer_view, &[0.0, 0.0, 0.0, 1.0]);
        }
        Ok(backbuffer)
    }

    /// 1 フレームを指定 backbuffer へコピーする (GPU 共有テクスチャ経路 / CPU upload 経路)。
    /// `present_reusing_surface` と `present_with_surface_swap` の両方から使う共通処理。
    fn copy_frame_into_backbuffer(
        &mut self,
        frame: &VideoFrame,
        backbuffer: &ID3D11Texture2D,
        surface_content: VideoSurfaceContent,
        target_width: u32,
        target_height: u32,
    ) -> Result<FrameCopyMetrics, String> {
        let mut metrics = FrameCopyMetrics {
            path: "",
            shared_handle: 0,
            shared_cache_hit: false,
            shared_texture_gen: 0,
            fence_value: 0,
            fence_wait_ms: 0.0,
            open_shared_ms: 0.0,
            keyed_mutex_ms: 0.0,
            keyed_mutex_cast_ms: 0.0,
            keyed_mutex_acquire_ms: 0.0,
            copy_call_ms: 0.0,
            copy_fence_value: 0,
        };
        let grade_active = !self.video_grade.adjustments.is_identity();
        let panorama_pose = if surface_content == VideoSurfaceContent::PanoramaResolved {
            Some(
                self.panorama_pose
                    .ok_or_else(|| "panorama surface has no active pose".to_string())?,
            )
        } else {
            None
        };
        let panorama_active = panorama_pose.is_some();
        let resample_active = surface_content == VideoSurfaceContent::DisplayResolved;
        let resample_mode = resample_active.then(|| {
            self.resample_mode(
                frame.width,
                frame.height,
                target_width,
                target_height,
                frame.orientation,
            )
            .expect("display-resolution surface always has a resample owner")
        });
        match &frame.data {
            VideoFrameData::Cpu(bytes) => {
                let probe_this_frame =
                    !grade_active && !panorama_active && !resample_active && self.pixel_probe_due();
                let src_probe = if probe_this_frame {
                    Some(sample_cpu_rgba_pixel(
                        bytes,
                        frame.width,
                        frame.height,
                        DXGI_FORMAT_B8G8R8A8_UNORM.0,
                    )?)
                } else {
                    None
                };
                copy_cpu_rgba_to_swapchain_bgra(
                    bytes,
                    &mut self.cpu_upload_scratch,
                    frame.width,
                    frame.height,
                )?;
                unsafe {
                    let copy_call_t0 = Instant::now();
                    if let Some((pose, uv)) = panorama_pose {
                        let source = self.grade_pipeline.upload_cpu_source(
                            &self.d3d_device1,
                            &self.d3d_context,
                            &self.cpu_upload_scratch,
                            frame.width,
                            frame.height,
                        )?;
                        let source = if grade_active {
                            self.grade_pipeline.draw_to_intermediate(
                                &self.d3d_device1,
                                &self.d3d_context,
                                &source,
                                frame.width,
                                frame.height,
                            )?
                        } else {
                            source
                        };
                        self.panorama_pipeline
                            .as_ref()
                            .ok_or_else(|| "video panorama pipeline is missing".to_string())?
                            .draw(
                                &self.d3d_device1,
                                &self.d3d_context,
                                &source,
                                backbuffer,
                                frame.width,
                                frame.height,
                                target_width,
                                target_height,
                                frame.orientation,
                                pose,
                                uv,
                            )?;
                    } else if resample_active {
                        let source = self.grade_pipeline.upload_cpu_source(
                            &self.d3d_device1,
                            &self.d3d_context,
                            &self.cpu_upload_scratch,
                            frame.width,
                            frame.height,
                        )?;
                        let source = if grade_active {
                            self.grade_pipeline.draw_to_intermediate(
                                &self.d3d_device1,
                                &self.d3d_context,
                                &source,
                                frame.width,
                                frame.height,
                            )?
                        } else {
                            source
                        };
                        self.resample_pipeline
                            .as_ref()
                            .ok_or_else(|| "video resample pipeline is missing".to_string())?
                            .draw(
                                &self.d3d_device1,
                                &self.d3d_context,
                                &source,
                                backbuffer,
                                frame.width,
                                frame.height,
                                target_width,
                                target_height,
                                frame.orientation,
                                resample_mode.expect("display resample mode"),
                            )?;
                    } else if grade_active {
                        let source = self.grade_pipeline.upload_cpu_source(
                            &self.d3d_device1,
                            &self.d3d_context,
                            &self.cpu_upload_scratch,
                            frame.width,
                            frame.height,
                        )?;
                        self.grade_pipeline.draw(
                            &self.d3d_device1,
                            &self.d3d_context,
                            &source,
                            backbuffer,
                            frame.width,
                            frame.height,
                        )?;
                    } else {
                        self.d3d_context.UpdateSubresource(
                            backbuffer,
                            0,
                            None,
                            self.cpu_upload_scratch.as_ptr().cast(),
                            frame.width.saturating_mul(4),
                            0,
                        );
                    }
                    metrics.copy_call_ms = copy_call_t0.elapsed().as_secs_f64() * 1000.0;
                    if probe_this_frame {
                        let backbuffer_probe =
                            self.sample_texture_pixel(backbuffer, "backbuffer")?;
                        self.log_pixel_probe(
                            "cpu_upload",
                            0,
                            0,
                            0.0,
                            src_probe,
                            Some(backbuffer_probe),
                        );
                        if self.pixel_probe_strict {
                            compare_pixel_probe(
                                "cpu_upload",
                                src_probe.unwrap(),
                                backbuffer_probe,
                            )?;
                        }
                    }
                }
                metrics.path = if panorama_active {
                    if grade_active {
                        "cpu_upload_grade_panorama"
                    } else {
                        "cpu_upload_panorama"
                    }
                } else {
                    match (resample_mode, grade_active) {
                        (Some(VideoResampleMode::Lanczos3 { .. }), true) => {
                            "cpu_upload_grade_resample_lanczos3"
                        }
                        (Some(VideoResampleMode::Lanczos3 { .. }), false) => {
                            "cpu_upload_resample_lanczos3"
                        }
                        (Some(VideoResampleMode::Nis), true) => "cpu_upload_grade_resample_nis",
                        (Some(VideoResampleMode::Nis), false) => "cpu_upload_resample_nis",
                        (Some(VideoResampleMode::Nearest), true) => {
                            "cpu_upload_grade_resample_nearest"
                        }
                        (Some(VideoResampleMode::Nearest), false) => "cpu_upload_resample_nearest",
                        (Some(VideoResampleMode::Anime4k { .. }), true) => {
                            "cpu_upload_grade_resample_anime4k"
                        }
                        (Some(VideoResampleMode::Anime4k { .. }), false) => {
                            "cpu_upload_resample_anime4k"
                        }
                        (None, true) => "cpu_upload_grade",
                        (None, false) => "cpu_upload",
                    }
                };
            }
            VideoFrameData::Gpu(gpu_frame) => {
                if gpu_frame.ten_bit {
                    return Err("10-bit D3D11 frame is not supported by native presenter".into());
                }
                if gpu_frame.width != frame.width || gpu_frame.height != frame.height {
                    return Err("D3D11 frame metadata size mismatch".into());
                }
                metrics.shared_handle = gpu_frame.shared_handle.0 as usize as u64;
                metrics.shared_texture_gen = gpu_frame.shared_texture_gen;
                metrics.fence_value = gpu_frame.fence_value;
                let probe_this_frame =
                    !grade_active && !panorama_active && !resample_active && self.pixel_probe_due();
                let fence = self.open_fence(gpu_frame.fence_gen, gpu_frame.fence_shared_handle)?;
                let fence_t0 = Instant::now();
                {
                    let _operation = self.health.begin_render_operation(
                        NativeRenderOperation::FenceWait,
                        self.window_epoch,
                    );
                    unsafe {
                        self.d3d_context4
                            .Wait(&fence, gpu_frame.fence_value)
                            .map_err(|e| format!("D3D11 fence wait: {e:?}"))?;
                    }
                }
                metrics.fence_wait_ms = fence_t0.elapsed().as_secs_f64() * 1000.0;
                let open_shared_t0 = Instant::now();
                let (src, cache_hit) = self
                    .open_shared_texture(gpu_frame.shared_handle, gpu_frame.shared_texture_gen)?;
                metrics.shared_cache_hit = cache_hit;
                metrics.open_shared_ms = open_shared_t0.elapsed().as_secs_f64() * 1000.0;
                let keyed_mutex_t0 = Instant::now();
                let keyed_mutex = self.acquire_source_keyed_mutex(
                    &src,
                    gpu_frame.shared_output_released_to_reader.clone(),
                )?;
                metrics.keyed_mutex_ms = keyed_mutex_t0.elapsed().as_secs_f64() * 1000.0;
                metrics.keyed_mutex_cast_ms = keyed_mutex.cast_ms;
                metrics.keyed_mutex_acquire_ms = keyed_mutex.acquire_ms;
                let keyed_mutex_guard = keyed_mutex.guard;
                let src_probe = if probe_this_frame {
                    Some(self.sample_texture_pixel(&src, "source")?)
                } else {
                    None
                };
                unsafe {
                    let copy_call_t0 = Instant::now();
                    if let Some((pose, uv)) = panorama_pose {
                        let source = if grade_active {
                            self.grade_pipeline.draw_to_intermediate(
                                &self.d3d_device1,
                                &self.d3d_context,
                                &src,
                                frame.width,
                                frame.height,
                            )?
                        } else {
                            src.clone()
                        };
                        self.panorama_pipeline
                            .as_ref()
                            .ok_or_else(|| "video panorama pipeline is missing".to_string())?
                            .draw(
                                &self.d3d_device1,
                                &self.d3d_context,
                                &source,
                                backbuffer,
                                frame.width,
                                frame.height,
                                target_width,
                                target_height,
                                frame.orientation,
                                pose,
                                uv,
                            )?;
                    } else if resample_active {
                        let source = if grade_active {
                            self.grade_pipeline.draw_to_intermediate(
                                &self.d3d_device1,
                                &self.d3d_context,
                                &src,
                                frame.width,
                                frame.height,
                            )?
                        } else {
                            src.clone()
                        };
                        self.resample_pipeline
                            .as_ref()
                            .ok_or_else(|| "video resample pipeline is missing".to_string())?
                            .draw(
                                &self.d3d_device1,
                                &self.d3d_context,
                                &source,
                                backbuffer,
                                frame.width,
                                frame.height,
                                target_width,
                                target_height,
                                frame.orientation,
                                resample_mode.expect("display resample mode"),
                            )?;
                    } else if grade_active {
                        self.grade_pipeline.draw(
                            &self.d3d_device1,
                            &self.d3d_context,
                            &src,
                            backbuffer,
                            frame.width,
                            frame.height,
                        )?;
                    } else {
                        let dst_res: ID3D11Resource = backbuffer
                            .cast()
                            .map_err(|e| format!("cast backbuffer resource: {e:?}"))?;
                        let src_res: ID3D11Resource = src
                            .cast()
                            .map_err(|e| format!("cast source resource: {e:?}"))?;
                        let copy_box = D3D11_BOX {
                            left: 0,
                            top: 0,
                            front: 0,
                            right: gpu_frame.width,
                            bottom: gpu_frame.height,
                            back: 1,
                        };
                        self.d3d_context.CopySubresourceRegion(
                            &dst_res,
                            0,
                            0,
                            0,
                            0,
                            &src_res,
                            0,
                            Some(&copy_box),
                        );
                    }
                    metrics.copy_call_ms = copy_call_t0.elapsed().as_secs_f64() * 1000.0;
                    if probe_this_frame {
                        let backbuffer_probe =
                            self.sample_texture_pixel(backbuffer, "backbuffer")?;
                        self.log_pixel_probe(
                            "d3d11_shared",
                            gpu_frame.fence_gen,
                            gpu_frame.fence_value,
                            metrics.fence_wait_ms,
                            src_probe,
                            Some(backbuffer_probe),
                        );
                        if self.pixel_probe_strict {
                            compare_pixel_probe(
                                "d3d11_shared",
                                src_probe.unwrap(),
                                backbuffer_probe,
                            )?;
                        }
                    }
                }
                metrics.path = if panorama_active {
                    if grade_active {
                        "d3d11_shared_grade_panorama"
                    } else {
                        "d3d11_shared_panorama"
                    }
                } else {
                    match (resample_mode, grade_active) {
                        (Some(VideoResampleMode::Lanczos3 { .. }), true) => {
                            "d3d11_shared_grade_resample_lanczos3"
                        }
                        (Some(VideoResampleMode::Lanczos3 { .. }), false) => {
                            "d3d11_shared_resample_lanczos3"
                        }
                        (Some(VideoResampleMode::Nis), true) => "d3d11_shared_grade_resample_nis",
                        (Some(VideoResampleMode::Nis), false) => "d3d11_shared_resample_nis",
                        (Some(VideoResampleMode::Nearest), true) => {
                            "d3d11_shared_grade_resample_nearest"
                        }
                        (Some(VideoResampleMode::Nearest), false) => {
                            "d3d11_shared_resample_nearest"
                        }
                        (Some(VideoResampleMode::Anime4k { .. }), true) => {
                            "d3d11_shared_grade_resample_anime4k"
                        }
                        (Some(VideoResampleMode::Anime4k { .. }), false) => {
                            "d3d11_shared_resample_anime4k"
                        }
                        (None, true) => "d3d11_shared_grade",
                        (None, false) => "d3d11_shared",
                    }
                };
                if let Some(guard) = keyed_mutex_guard {
                    guard.release_to_reader()?;
                }
            }
        }
        // フレームコピー (UpdateSubresource / CopySubresourceRegion) を GPU タイムライン上で
        // 待ち合わせるための fence signal。`run_native_video_output` 側の `present_retire` が
        // `copy_fence_completed_value()` でこの値の到達を見て、コピーが GPU 上で完了した
        // フレームだけ共有出力 slot を解放する (= presenter のコピー完了前に producer が
        // slot を再利用して上書きするレースを構造的に塞ぐ)。
        if let Some(fence) = self.copy_fence.as_ref() {
            self.copy_fence_value += 1;
            let value = self.copy_fence_value;
            unsafe {
                match self.d3d_context4.Signal(fence, value) {
                    Ok(()) => metrics.copy_fence_value = value,
                    Err(e) => crate::logger::log(format!(
                        "native-presenter: copy fence Signal({value}) failed: {e:?}"
                    )),
                }
            }
        }
        Ok(metrics)
    }

    /// presenter copy fence の現在の完了値。`run_native_video_output` の `present_retire` が
    /// これを見て「コピーが GPU 上で完了したフレーム」を判定する。fence 未作成時は
    /// `u64::MAX` を返さず `None` を返し、呼び出し側は depth キャップのみで運用する。
    pub fn copy_fence_completed_value(&self) -> Option<u64> {
        self.copy_fence
            .as_ref()
            .map(|fence| unsafe { fence.GetCompletedValue() })
    }

    /// `shared_texture_cache` を全エントリ破棄する。各 entry は D3D11 `OpenSharedResource1`
    /// で開いた共有 texture を保持しており、4K で約 32 MB / 1080p で 8 MB を adapter
    /// memory に占有する。動画切替 (`SwitchSource`) 時は前動画の texture を残しても
    /// 二度と参照されないので即時破棄する (Codex 助言、2026-05-15)。残ったエントリは
    /// 次の `open_shared_texture` 呼出で再生成される (1-2ms オーバーヘッド、許容範囲)。
    pub fn clear_shared_texture_cache(&mut self) {
        let cleared = self.shared_texture_cache.len();
        self.shared_texture_cache.clear();
        if cleared > 0 {
            crate::logger::log(format!(
                "[native-presenter] shared_texture_cache cleared on source switch (entries={cleared})"
            ));
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "native_presenter",
                    "shared_texture_cache_cleared",
                    None,
                    0,
                    &[("entries", serde_json::Value::from(cleared as i64))],
                );
            }
        }
    }

    pub fn shared_texture_cache_len(&self) -> usize {
        self.shared_texture_cache.len()
    }

    pub fn retired_video_surface_len(&self) -> usize {
        self.retired_video_surfaces.len()
    }

    pub fn surface_size(&self) -> (u32, u32) {
        (self.surface_width, self.surface_height)
    }

    pub fn intermediate_vram_bytes(&self) -> (u64, u64) {
        (
            self.grade_pipeline.intermediate_vram_bytes(),
            self.resample_pipeline
                .as_ref()
                .map(VideoResamplePipeline::intermediate_vram_bytes)
                .unwrap_or(0),
        )
    }

    /// HUD HWND の `WM_DPICHANGED` を受けて overlay の OS ppp を更新する。
    /// `dirty = true` 化されるので次フレームの render で region 物理ピクセル換算が新 DPI 基準になる。
    /// 戻り値: 値が変わったかどうか。
    pub fn set_overlay_pixels_per_point(&mut self, os_ppp: f32) -> Result<bool, String> {
        let changed = self
            .egui_overlay
            .as_mut()
            .map(|o| o.set_os_pixels_per_point(os_ppp))
            .unwrap_or(false);
        if changed {
            self.update_video_visual_transform(self.width, self.height)?;
        }
        Ok(changed)
    }

    pub(crate) fn set_window_observation(&mut self, observation: NativeWindowObservation) {
        self.window_observation = observation;
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.window_observation = observation;
        }
    }

    /// **overlay (egui_wgpu) の surface だけ** を resize する。
    /// presenter 全体 (= background / video transform / swap chain) は触らない。
    ///
    /// `DpiChanged` 経由で HUD HWND の `suggested_rect` に合わせて overlay surface を
    /// resize するときに使う。presenter HWND は別 monitor / 別 DPI 経路 (`WM_DPICHANGED`)
    /// で別途 resize される (現状未配線)。HUD の suggested_rect で presenter video まで
    /// 引っ張られると video transform が壊れるので、専用経路に分ける。
    ///
    /// 通常の `resize(width, height)` は presenter 全体を resize するので、
    /// `WM_WINDOWPOSCHANGED` 経由で presenter HWND 自身の geometry が変わったときに使う。
    pub(crate) fn resize_overlay_surface_only(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<Vec<NativeWindowIntent>, String> {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            return overlay.resize(width, height);
        }
        Ok(Vec::new())
    }

    /// presenter thread loop から 50ms 周期で呼ばれる HUD hover polling。
    /// 戻り値: `true` なら呼び出し側で `raise_hud_to_top()` + retry burst を起動すべき。
    /// HUD HWND が無い (= フォールバック経路) なら何もせず `false`。
    ///
    /// 役割:
    ///   1. pump の `NativeWindowObservation` から cursor の presenter 座標を取得。
    ///   2. pump-owned router の input ownership と client 座標の範囲を確認し、
    ///      範囲外なら一度だけ synthetic `MouseLeave` を流して以降何もしない。
    ///   3. 範囲内: 直近 80ms 以内に HUD/presenter wndproc 経由の本物 `WM_MOUSEMOVE` が
    ///      届いていなければ synthetic `MouseMove` を overlay の `push_native_event` に流す
    ///      (= region 外 cursor でも hover 表示遷移を成立させる)。
    ///   4. cursor が activation zone (= 上端 0..76pt / 下端 H-220..H pt) 内、かつ
    ///      `editor_hwnds_snapshot` から `foreground_allows_hud_raise` が true を返した場合、
    ///      raise を要求する (= 戻り値 true)。判定不能なら false で skip。
    pub(crate) fn cursor_polling_tick(
        &mut self,
        last_native_mouse_at: Option<std::time::Instant>,
        pointer_present_synthetic: &mut bool,
    ) -> bool {
        // 実機修正 (2026-05-12): 外部 drag 検出。
        // pump snapshot で global left button が down なら左ボタン押下中。
        // ただし HUD region 内クリックは egui の `pointer.any_down()` でも true になる
        // (= 内部 drag、e.g. seek bar の drag)。両方の差分で「HUD 外で down している」
        // = 外部 drag (e.g. VST window のドラッグ) を判定する。
        //
        // ## 実機修正 (2026-05-12 P1 #3): 100ms delay を入れる
        //
        // 旧版は LBUTTON DOWN 直後のフレームで即 external_drag = true と判定していたが、
        // これだと「user が HUD top bar の button をクリックしたフレーム」と「event が
        // egui に届くフレーム」の間に polling が走って external_drag flip → top bar 非表示
        // になり、click event の処理時には button が描画されておらず click が失われる
        // (= ユーザー報告「VST ボタンを押しても反応しない、一瞬上のホバーバーが消える」)。
        //
        // LBUTTON DOWN を検出した最初のフレームでは `lbutton_down_since` だけ記録し、
        // external_drag は false のまま (= bar 表示維持)。100ms 経過後も DOWN 継続中で
        // egui に any_down が伝わっていなければ「真の外部 drag」と判定する。
        // 通常 click は数 ms ~ 数十 ms で UP するので、この delay で click は誤検出されない。
        let observation = self.window_observation;
        let global_lbutton_down = observation.global_lbutton_down;
        let egui_pointer_down = self
            .egui_overlay
            .as_ref()
            .map(|o| o.egui_ctx.input(|i| i.pointer.any_down()))
            .unwrap_or(false);
        if global_lbutton_down {
            if self.lbutton_down_since.is_none() {
                self.lbutton_down_since = Some(std::time::Instant::now());
            }
        } else {
            self.lbutton_down_since = None;
        }
        let lbutton_down_long_enough = self
            .lbutton_down_since
            .map(|t| t.elapsed() >= std::time::Duration::from_millis(100))
            .unwrap_or(false);
        // Codex P2 #5 反映: HUD HWND が `SetCapture` で mouse を取っている間は HUD 起点クリック
        // (= seek drag や bar button hold) なので external_drag に分類しない。これで 100ms heuristic
        // を抜けた長押し操作 (e.g. seek bar drag が長引くケース) でも top bar を消さない。
        let external_drag = global_lbutton_down
            && !egui_pointer_down
            && lbutton_down_long_enough
            && !observation.hud_has_capture;
        if let Some(overlay) = self.egui_overlay.as_mut() {
            if overlay.external_drag_in_progress != external_drag {
                overlay.external_drag_in_progress = external_drag;
                // 状態変化 → 次フレームで hover 判定が変わるので render dirty 化
                overlay.dirty = true;
            }
        }

        // HUD HWND が無い経路 (= CP4 フォールバック / CP7 で flip 前) は polling 不要。
        if !observation.has_hud {
            return false;
        }
        let Some([x, y]) = observation.cursor_client_position else {
            return false;
        };
        let in_range = observation.cursor_input_owned
            && x >= 0
            && y >= 0
            && (x as u32) < self.width
            && (y as u32) < self.height;

        if !in_range {
            // 範囲外: overlay に残っている pointer_pos を 1 度だけ clear して終了。
            // 最後の mouse move が HUD HWND 由来の real event だった場合、
            // pointer_present_synthetic は false のままなので、それだけを見ると
            // right panel / seek HUD が stale hover で残り続ける。
            let overlay_has_pointer = self
                .egui_overlay
                .as_ref()
                .is_some_and(NativeEguiOverlay::has_pointer_pos);
            if overlay_has_pointer {
                if hud_debug_enabled() {
                    crate::logger::log(
                        "[HUD-DEBUG] polling MouseLeave (cursor out of client rect)".to_string(),
                    );
                }
                if let Some(overlay) = self.egui_overlay.as_mut() {
                    overlay.push_native_event(
                        crate::video::native_window::NativeVideoWindowEvent::MouseLeave,
                    );
                }
            }
            *pointer_present_synthetic = false;
            return false;
        }

        // 範囲内: 直近 80ms 以内に本物 mouse が届いていなければ synthetic MouseMove。
        // Codex CP6 再 P3 反映: `pointer_present_synthetic` は **synthetic を実際に push
        // した時のみ** `true` にする。recent_native で skip した場合はフラグを動かさない
        // (= 直前に synthetic だったなら true のまま、本物経路だけなら false のまま)。
        // これにより client rect 外に出たとき、native `WM_MOUSELEAVE` と二重で synthetic
        // `MouseLeave` を流すリスクを排除する (= 「synthetic を流した状態」だけが
        // synthetic leave を必要とする)。
        let now = std::time::Instant::now();
        let recent_native = last_native_mouse_at
            .is_some_and(|t| now.duration_since(t) < std::time::Duration::from_millis(80));
        let needs_synthetic_move = self
            .egui_overlay
            .as_ref()
            .is_some_and(|overlay| overlay.needs_synthetic_pointer_move(x, y));
        if !recent_native && needs_synthetic_move {
            if hud_debug_enabled() {
                crate::logger::log(format!(
                    "[HUD-DEBUG] polling synthetic MouseMove x={} y={} (no native mouse in 80ms)",
                    x, y
                ));
            }
            if let Some(overlay) = self.egui_overlay.as_mut() {
                overlay.push_native_event(
                    crate::video::native_window::NativeVideoWindowEvent::MouseMove(
                        crate::video::native_window::NativeVideoMouseEvent {
                            x,
                            y,
                            shift: false,
                            ctrl: false,
                        },
                    ),
                );
                *pointer_present_synthetic = true;
            }
        }
        // recent_native / unchanged-position の場合は `pointer_present_synthetic` を変更しない
        // (前回値維持)。同じ座標の PointerMoved を投げ続けると egui の tooltip
        // delay が毎回リセットされ、HUD tooltips が出なくなる。

        // activation zone 判定 (= 上端 76pt 帯 / 下端 220pt 帯)。
        // pixels_per_point は overlay から取得 (= CP3 で導入した overlay の pixels_per_point)。
        let ppp = self
            .egui_overlay
            .as_ref()
            .map(|o| o.pixels_per_point)
            .unwrap_or(1.0);
        let top_band_px = (76.0_f32 * ppp).round() as i32;
        let bottom_band_top = self.height as i32 - (220.0_f32 * ppp).round() as i32;
        let in_activation_zone = y < top_band_px || y >= bottom_band_top;
        if !in_activation_zone {
            return false;
        }

        true
    }

    pub fn handle_window_events(
        &mut self,
        events: &[crate::video::native_window::NativeVideoWindowEvent],
    ) -> Result<NativeOverlayInputOutcome, String> {
        let outcome = if let Some(overlay) = self.egui_overlay.as_mut() {
            let modal_dialog_active_before_events = overlay.modal_dialog_active_for_routing();
            overlay.push_native_events(events);
            let mut outcome = overlay.render_if_dirty()?;
            outcome.routing.modal_dialog_active |= modal_dialog_active_before_events;
            outcome
        } else {
            NativeOverlayInputOutcome::empty()
        };
        Ok(outcome)
    }

    pub(crate) fn fullscreen_overlay_active(&self) -> bool {
        self.egui_overlay.as_ref().is_some_and(|overlay| {
            overlay.navigation_preview.is_some()
                || overlay.tile_overlay.is_some()
                || overlay.seek_strip.is_some()
                || overlay.native_touch.first_run_help_visible()
        })
    }

    pub(crate) fn overlay_toast_active(&self) -> bool {
        self.egui_overlay
            .as_ref()
            .is_some_and(|overlay| overlay.toast.is_some())
    }

    pub(crate) fn hud_debug_description(&self, regions: &[RECT]) -> Option<String> {
        if !hud_debug_enabled() {
            return None;
        }
        let overlay = self.egui_overlay.as_ref()?;
        let rects = regions
            .iter()
            .map(|rect| {
                format!(
                    "({},{} {}x{})",
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        Some(format!(
            "[HUD-DEBUG] regions changed n={} ptr={:?} top_bar={} hud_vis={} right={} jump={} vst3={} thumb={} pin={} rects=[{}]",
            regions.len(),
            overlay.pointer_pos,
            overlay.top_bar_visible,
            overlay.hud_visible(),
            overlay.right_panel_visible(),
            overlay.jump_panel_visible,
            overlay.vst3_panel_visible(),
            overlay.hover_thumbnail.is_some(),
            overlay.hover_preview_pinned,
            rects,
        ))
    }
    pub fn update_overlay_video_state(
        &mut self,
        position_secs: f64,
        duration_secs: f64,
        is_playing: bool,
        volume: f64,
        muted: bool,
        limiter_ceiling_hit_seq: u64,
        playback_speed: f64,
        frame_step_active: bool,
        is_seeking: bool,
        seek_serial: u64,
    ) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.update_video_state(
                position_secs,
                duration_secs,
                is_playing,
                volume,
                muted,
                limiter_ceiling_hit_seq,
                playback_speed,
                frame_step_active,
                is_seeking,
                seek_serial,
            );
        }
    }

    pub fn set_overlay_perf_visible(&mut self, visible: bool) -> bool {
        self.egui_overlay
            .as_mut()
            .map(|overlay| overlay.set_perf_visible(visible))
            .unwrap_or(false)
    }

    pub fn push_overlay_perf_sample(
        &mut self,
        sample: NativeOverlayPerfSample,
        snapshot: NativeOverlayPerfSnapshot,
    ) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.push_perf_sample(sample, snapshot);
        }
    }

    /// 動画ソース切替時に perf overlay の履歴 / pause gap pending / 最新スナップショットを
    /// クリアする。`SwitchSource` ハンドラの周辺 reset 群と同じパターン。詳細は
    /// `NativeEguiOverlay::reset_perf` の doc コメント参照。
    pub fn reset_overlay_perf(&mut self) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.reset_perf();
        }
    }

    pub fn set_overlay_hover_thumbnail(&mut self, thumbnail: Option<NativeOverlayThumbnail>) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_hover_thumbnail(thumbnail);
        }
    }

    pub fn set_overlay_hover_preview_pinned(&mut self, pinned: bool) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_hover_preview_pinned(pinned);
        }
    }

    pub fn set_overlay_timeline_markers(&mut self, markers: Vec<NativeOverlayTimelineMarker>) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_timeline_markers(markers);
        }
    }

    pub fn set_overlay_jump_entries(&mut self, entries: Vec<NativeOverlayJumpEntry>) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_jump_entries(entries);
        }
    }

    pub fn set_video_grade(
        &mut self,
        mut grade: crate::creative_lut::VideoGradeSnapshot,
    ) -> Result<(), String> {
        grade.adjustments.sanitize();
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_video_grade(&grade);
        }
        self.video_grade = grade;
        self.grade_pipeline.update_grade(
            &self.d3d_device1,
            &self.d3d_context,
            &self.video_grade,
        )?;
        Ok(())
    }

    pub fn set_panorama_pose(
        &mut self,
        panorama_pose: Option<(PanoPose, PanoUvTransform)>,
    ) -> Result<(), String> {
        self.panorama_pose = panorama_pose;
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_panorama_pose(panorama_pose.map(|(pose, _)| pose));
        }
        self.sync_overlay_video_scale_state();
        Ok(())
    }

    pub fn set_overlay_metadata(&mut self, metadata: Option<NativeOverlayMetadata>) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_metadata(metadata);
        }
    }

    pub(crate) fn set_overlay_side_panel_state(
        &mut self,
        mode: FsSidePanelMode,
        info_panel_open: crate::ui_helpers::MetadataPanelOpenState,
    ) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_side_panel_state(mode, info_panel_open);
        }
    }

    pub(crate) fn set_overlay_bar_lock_state(
        &mut self,
        top_locked: bool,
        bottom_lock: VideoBottomLock,
        fixed_bar_gap_px: u32,
    ) -> Result<(), String> {
        let fixed_bar_gap_px =
            fixed_bar_gap_px.min(crate::settings::FULLSCREEN_FIXED_BAR_GAP_MAX_PX);
        let layout_changed = self.video_top_bar_locked != top_locked
            || self.video_bottom_lock != bottom_lock
            || self.fullscreen_fixed_bar_gap_px != fixed_bar_gap_px;
        self.video_top_bar_locked = top_locked;
        self.video_bottom_lock = bottom_lock;
        self.fullscreen_fixed_bar_gap_px = fixed_bar_gap_px;
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_bar_lock_state(top_locked, bottom_lock);
        }
        if layout_changed {
            self.update_video_visual_transform(self.width, self.height)?;
        }
        Ok(())
    }

    /// Source swap で presenter-local な panel/touch session を閉じる。
    /// 右状態は App が新ファイルの false を別 command で同期する。
    pub fn reset_overlay_source_session(&mut self) -> Result<bool, String> {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.reset_side_panel_session();
            overlay.reset_seek_preview_source_session();
        }
        self.set_overlay_seek_strip(
            None,
            crate::video::seek_strip::SeekStripMaterialAvailability::Unknown,
        )
    }

    /// Video/audio mode changes use the same session boundary as source swap.
    pub fn reset_overlay_side_panel_session(&mut self) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.reset_side_panel_session();
        }
    }

    pub fn set_overlay_fallback_file_name(&mut self, file_name: String) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_fallback_file_name(file_name);
        }
    }

    pub fn set_overlay_tile_overlay(&mut self, tile_overlay: Option<NativeOverlayTileOverlay>) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_tile_overlay(tile_overlay);
        }
    }

    pub(crate) fn set_overlay_seek_strip(
        &mut self,
        seek_strip: Option<NativeOverlaySeekStrip>,
        material_availability: crate::video::seek_strip::SeekStripMaterialAvailability,
    ) -> Result<bool, String> {
        let was_visible = self
            .egui_overlay
            .as_ref()
            .is_some_and(|overlay| overlay.seek_strip.is_some());
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_seek_strip(seek_strip, material_availability);
        }
        let is_visible = self
            .egui_overlay
            .as_ref()
            .is_some_and(|overlay| overlay.seek_strip.is_some());
        let layout_changed = was_visible != is_visible && self.video_bottom_lock.strip_locked();
        if layout_changed {
            self.update_video_visual_transform(self.width, self.height)?;
        }
        Ok(layout_changed)
    }

    pub fn set_overlay_ring_picker(&mut self, picker: Option<NativeOverlayRingPicker>) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_ring_picker_overlay(picker);
        }
    }

    pub fn set_overlay_ring_guide(&mut self, guide: Option<NativeOverlayRingGuide>) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_ring_guide_overlay(guide);
        }
    }

    pub fn set_overlay_navigation_preview(
        &mut self,
        preview: Option<NativeOverlayNavigationPreview>,
    ) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_navigation_preview(preview);
        }
    }

    pub fn set_overlay_loop_enabled(&mut self, enabled: bool) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_loop_enabled(enabled);
        }
    }

    /// HUD ボタン表示用のループモード (= ユーザー設定の display_mode) を overlay に伝える。
    pub fn set_overlay_loop_mode(&mut self, mode: crate::settings::VideoLoopMode) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_loop_mode(mode);
        }
    }

    pub fn set_overlay_continuous_mode(&mut self, mode: crate::video::VideoContinuousMode) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_continuous_mode(mode);
        }
    }

    pub fn set_overlay_checked(&mut self, checked: bool) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_checked(checked);
        }
    }

    pub fn set_overlay_vst3_available(&mut self, available: bool) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_vst3_available(available);
        }
    }

    pub fn set_overlay_hud_dimmed(&mut self, dimmed: bool) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_hud_dimmed(dimmed);
        }
    }

    pub fn set_overlay_text_contrast(&mut self, contrast: crate::settings::TextContrast) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_text_contrast(contrast);
        }
    }

    pub fn set_overlay_audio_only(&mut self, audio_only: bool) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_audio_only(audio_only);
        }
    }

    pub fn set_overlay_vst3_panel(&mut self, panel: Option<NativeOverlayVst3Panel>) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_vst3_panel(panel);
        }
    }

    pub fn set_overlay_playback_status(
        &mut self,
        first_frame_presented: bool,
        error: Option<String>,
        prep_status: crate::video::avio_progress::PreparingStatus,
    ) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_playback_status(first_frame_presented, error, prep_status);
        }
    }

    /// 音量ノーマライズ UI 状態を overlay に配信。boutton 色 + 進捗パネル描画に使う。
    pub fn set_overlay_normalize_state(
        &mut self,
        state: crate::video::normalize_types::NormalizeOverlayState,
    ) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.set_normalize_state(state);
        }
    }

    /// eframe 経由でルーティングされた pointer 活動を overlay に伝搬する。
    /// `push_native_event` を経由しないため明示的に呼ぶ必要がある。
    pub fn mark_overlay_cursor_activity(&mut self) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.mark_cursor_activity();
        }
    }

    pub fn request_overlay_render(&mut self) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.request_render();
        }
    }

    pub fn show_overlay_toast(
        &mut self,
        text: String,
        centered: bool,
        linger: Option<std::time::Duration>,
    ) {
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.show_toast(text, centered, linger);
        }
    }

    pub fn set_pixel_probe(&mut self, enabled: bool, strict: bool) {
        self.pixel_probe_enabled = enabled;
        self.pixel_probe_strict = strict;
        self.last_pixel_probe = None;
    }

    pub fn tick_overlay_video_state(
        &mut self,
        position_secs: f64,
        duration_secs: f64,
        is_playing: bool,
        volume: f64,
        muted: bool,
        limiter_ceiling_hit_seq: u64,
        playback_speed: f64,
        frame_step_active: bool,
        is_seeking: bool,
        seek_serial: u64,
    ) -> Result<NativeOverlayInputOutcome, String> {
        let outcome = if let Some(overlay) = self.egui_overlay.as_mut() {
            let force_tick_render =
                overlay.wants_periodic_tick() || overlay.repaint_due(Instant::now());
            overlay.update_video_state(
                position_secs,
                duration_secs,
                is_playing,
                volume,
                muted,
                limiter_ceiling_hit_seq,
                playback_speed,
                frame_step_active,
                is_seeking,
                seek_serial,
            );
            if force_tick_render {
                overlay.dirty = true;
            }
            overlay.render_if_dirty()?
        } else {
            NativeOverlayInputOutcome::empty()
        };
        // Codex CP5 P1 反映: tick 経路でも overlay UI が時間経過で表示/非表示に変わるので、
        // 必ず HUD region に反映する。漏らすと periodic 表示状態 (= toast / hover preview /
        // tile overlay refresh 等) と region がズレて VST にクリックが奪われる。
        Ok(outcome)
    }

    pub fn overlay_hud_visible(&self) -> bool {
        self.egui_overlay
            .as_ref()
            .map(NativeEguiOverlay::hud_visible)
            .unwrap_or(false)
    }

    pub fn overlay_wants_periodic_tick(&self) -> bool {
        self.egui_overlay
            .as_ref()
            .map(NativeEguiOverlay::wants_periodic_tick)
            .unwrap_or(false)
    }

    pub fn overlay_repaint_due(&self, now: Instant) -> bool {
        self.egui_overlay
            .as_ref()
            .is_some_and(|overlay| overlay.repaint_due(now))
    }

    pub fn overlay_needs_render(&self) -> bool {
        self.egui_overlay
            .as_ref()
            .map(NativeEguiOverlay::needs_render)
            .unwrap_or(false)
    }

    fn open_fence(
        &mut self,
        fence_gen: u64,
        fence_shared_handle: HANDLE,
    ) -> Result<ID3D11Fence, String> {
        let handle_key = fence_shared_handle.0 as isize;
        let needs_open = !matches!(
            self.fence_cache,
            Some((cached_gen, cached_handle, _)) if cached_gen == fence_gen && cached_handle == handle_key
        );
        if needs_open {
            let mut fence = None;
            unsafe {
                self.d3d_device5
                    .OpenSharedFence(fence_shared_handle, &mut fence)
                    .map_err(|e| format!("OpenSharedFence: {e:?}"))?;
            }
            let fence = fence.ok_or_else(|| "OpenSharedFence returned null".to_string())?;
            self.fence_cache = Some((fence_gen, handle_key, fence));
        }
        Ok(self.fence_cache.as_ref().unwrap().2.clone())
    }

    /// 共有出力テクスチャを `OpenSharedResource1` で開く。結果は `(handle 値, gen)` を
    /// キーにキャッシュする。
    ///
    /// ⚠️ **`gen` をキーに含めるのが必須**: NT shared handle の値は、decoder 側で
    /// 共有出力 slot を evict (`CloseHandle`) した後に OS が別の slot 用へ再利用しうる。
    /// handle 値だけでキャッシュすると、動画切替で「前動画のテクスチャ」を stale なまま
    /// 返してしまい、新動画の再生中に前動画のフレームが 1 枚混入する (2026-05-15 報告の
    /// frame 225)。`gen` (`SharedOutputSlot::texture_gen`、プロセス内ユニーク・単調増加)
    /// を組にすることで、handle 値が再利用されても別エントリとして必ず開き直す。
    /// `open_fence` の `fence_gen` と同じ防御。
    fn open_shared_texture(
        &mut self,
        shared_handle: HANDLE,
        shared_texture_gen: u64,
    ) -> Result<(ID3D11Texture2D, bool), String> {
        let handle_key = shared_handle.0 as usize as u64;
        let cache_key = (handle_key, shared_texture_gen);
        if let Some(pos) = self
            .shared_texture_cache
            .iter()
            .position(|(cached_key, _)| *cached_key == cache_key)
        {
            let texture = self.shared_texture_cache[pos].1.clone();
            if pos != 0 {
                let entry = self.shared_texture_cache.remove(pos);
                self.shared_texture_cache.insert(0, entry);
            }
            return Ok((texture, true));
        }

        let texture: ID3D11Texture2D = unsafe {
            self.d3d_device1
                .OpenSharedResource1(shared_handle)
                .map_err(|e| format!("OpenSharedResource1 frame texture: {e:?}"))?
        };
        self.shared_texture_cache
            .insert(0, (cache_key, texture.clone()));
        if self.shared_texture_cache.len() > SHARED_TEXTURE_CACHE_CAPACITY {
            self.shared_texture_cache.pop();
        }
        Ok((texture, false))
    }

    /// `present_with_surface_swap` の `Commit` 直後に呼ぶ。content + transform の Commit が
    /// composition engine に処理され切るまで presenter thread を待たせる。これにより
    /// 旧 swap chain を `retired_video_surfaces` へ移して以降の present に進む時点で、
    /// DComp は確実に新 swap chain を指している。
    ///
    /// `WaitForCommitCompletion` だけでは、実機で 3840→1920 のような縮小 swap 時に
    /// 新 content が旧 transform で 1 refresh 見えることがあった。黒フレームは挿入せず、
    /// `DwmFlush` で DWM 側の表示反映まで同期して content / transform のペアを固定する。
    /// 戻り値は待ちに要した時間 (ms)。
    fn wait_for_video_transform_commit(&self) -> f64 {
        let t0 = Instant::now();
        unsafe {
            let _ = self._dcomp_device.WaitForCommitCompletion();
            let _ = DwmFlush();
        }
        t0.elapsed().as_secs_f64() * 1000.0
    }

    /// `win_w` / `win_h` は明示引数。`self.width` / `self.height` を直接読まないのは、
    /// `resize()` が `self.width` を**末尾で**更新するため (= 呼び出し時点では 1 resize
    /// 前の値)。stale なサイズで transform を計算すると最大化/復元/ドラッグの追従が
    /// 1 ステップ遅れる (2026-05 修正)。
    fn update_video_visual_transform(&self, win_w: u32, win_h: u32) -> Result<(), String> {
        let (sar_num, sar_den, orientation) = transform_geometry_for_surface(
            self.surface_content,
            self.sar_num,
            self.sar_den,
            self.orientation,
        );
        let (m11, m12, m21, m22, offset_x, offset_y) = compute_video_visual_transform_for_surface(
            self.surface_width,
            self.surface_height,
            win_w,
            win_h,
            sar_num,
            sar_den,
            orientation,
            self.video_visual_layout(),
            self.surface_content == VideoSurfaceContent::PanoramaResolved,
        );
        let transform = Matrix3x2 {
            M11: m11,
            M12: m12,
            M21: m21,
            M22: m22,
            M31: offset_x,
            M32: offset_y,
        };
        let _operation = self
            .health
            .begin_render_operation(NativeRenderOperation::DCompCommit, self.window_epoch);
        unsafe {
            self._video_visual
                .SetTransform2(&transform)
                .map_err(|e| format!("IDCompositionVisual::SetTransform2 video: {e:?}"))?;
            self._dcomp_device
                .Commit()
                .map_err(|e| format!("IDCompositionDevice::Commit video transform: {e:?}"))?;
        }
        Ok(())
    }

    fn video_visual_layout(&self) -> VideoVisualLayout {
        VideoVisualLayout {
            compact: self.video_compact,
            pixels_per_point: self
                .egui_overlay
                .as_ref()
                .map(|overlay| overlay.pixels_per_point)
                .unwrap_or(1.0),
            top_bar_locked: self.video_top_bar_locked,
            bottom_lock: self.video_bottom_lock,
            seek_strip_visible: self
                .egui_overlay
                .as_ref()
                .is_some_and(|overlay| overlay.seek_strip.is_some()),
            fixed_bar_gap_px: self.fullscreen_fixed_bar_gap_px,
        }
    }

    fn pixel_probe_due(&mut self) -> bool {
        if !self.pixel_probe_enabled {
            return false;
        }
        let now = Instant::now();
        let due = self
            .last_pixel_probe
            .map(|last| now.duration_since(last) >= Duration::from_secs(1))
            .unwrap_or(true);
        if due {
            self.last_pixel_probe = Some(now);
        }
        due
    }

    fn acquire_source_keyed_mutex(
        &self,
        texture: &ID3D11Texture2D,
        released_to_reader: Option<Arc<AtomicBool>>,
    ) -> Result<SourceKeyedMutexAcquire, String> {
        let cast_t0 = Instant::now();
        let Ok(mutex) = texture.cast::<IDXGIKeyedMutex>() else {
            return Ok(SourceKeyedMutexAcquire {
                guard: None,
                cast_ms: cast_t0.elapsed().as_secs_f64() * 1000.0,
                acquire_ms: 0.0,
            });
        };
        let cast_ms = cast_t0.elapsed().as_secs_f64() * 1000.0;
        let acquire_t0 = Instant::now();
        {
            let _operation = self
                .health
                .begin_render_operation(NativeRenderOperation::AcquireSync, self.window_epoch);
            unsafe {
                mutex
                    .AcquireSync(1, 10)
                    .map_err(|e| format!("source keyed mutex AcquireSync(1): {e:?}"))?;
            }
        }
        let acquire_ms = acquire_t0.elapsed().as_secs_f64() * 1000.0;
        Ok(SourceKeyedMutexAcquire {
            guard: Some(KeyedMutexReadGuard {
                mutex,
                released_to_reader,
                armed: true,
            }),
            cast_ms,
            acquire_ms,
        })
    }

    fn sample_texture_pixel(
        &self,
        texture: &ID3D11Texture2D,
        label: &str,
    ) -> Result<NativePixelSample, String> {
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            if desc.Width == 0 || desc.Height == 0 {
                return Err(format!("pixel probe {label}: empty texture desc"));
            }
            if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
                return Err(format!(
                    "pixel probe {label}: unsupported format {:?}",
                    desc.Format
                ));
            }

            let x = desc.Width / 2;
            let y = desc.Height / 2;
            let staging_desc = D3D11_TEXTURE2D_DESC {
                Width: 1,
                Height: 1,
                MipLevels: 1,
                ArraySize: 1,
                Format: desc.Format,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut staging = None;
            self.d3d_device1
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))
                .map_err(|e| format!("pixel probe {label}: CreateTexture2D staging: {e:?}"))?;
            let staging =
                staging.ok_or_else(|| format!("pixel probe {label}: staging texture null"))?;
            let src_res: ID3D11Resource = texture
                .cast()
                .map_err(|e| format!("pixel probe {label}: cast source: {e:?}"))?;
            let staging_res: ID3D11Resource = staging
                .cast()
                .map_err(|e| format!("pixel probe {label}: cast staging: {e:?}"))?;
            let src_box = D3D11_BOX {
                left: x,
                top: y,
                front: 0,
                right: x.saturating_add(1),
                bottom: y.saturating_add(1),
                back: 1,
            };
            self.d3d_context.CopySubresourceRegion(
                &staging_res,
                0,
                0,
                0,
                0,
                &src_res,
                0,
                Some(&src_box),
            );

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.d3d_context
                .Map(&staging_res, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|e| format!("pixel probe {label}: Map staging: {e:?}"))?;
            let ptr = mapped.pData.cast::<u8>();
            let sample = NativePixelSample {
                x,
                y,
                width: desc.Width,
                height: desc.Height,
                format: desc.Format.0,
                b: *ptr,
                g: *ptr.add(1),
                r: *ptr.add(2),
                a: *ptr.add(3),
            };
            self.d3d_context.Unmap(&staging_res, 0);
            Ok(sample)
        }
    }

    fn log_pixel_probe(
        &self,
        path: &str,
        fence_gen: u64,
        fence_value: u64,
        fence_wait_ms: f64,
        source: Option<NativePixelSample>,
        backbuffer: Option<NativePixelSample>,
    ) {
        let src = source.unwrap_or(NativePixelSample {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            format: 0,
            b: 0,
            g: 0,
            r: 0,
            a: 0,
        });
        let dst = backbuffer.unwrap_or(src);
        crate::logger::log(format!(
            "native-presenter: pixel_probe path={path} fence_gen={fence_gen} fence_value={fence_value} \
             fence_wait_ms={fence_wait_ms:.3} src@{},{} size={}x{} fmt={} bgra=({},{},{},{}) \
             backbuffer@{},{} size={}x{} fmt={} bgra=({},{},{},{})",
            src.x,
            src.y,
            src.width,
            src.height,
            src.format,
            src.b,
            src.g,
            src.r,
            src.a,
            dst.x,
            dst.y,
            dst.width,
            dst.height,
            dst.format,
            dst.b,
            dst.g,
            dst.r,
            dst.a,
        ));
        log_event(
            "pixel_probe",
            &[
                ("path", Value::from(path)),
                ("fence_gen", Value::from(fence_gen as i64)),
                ("fence_value", Value::from(fence_value as i64)),
                ("fence_wait_ms", Value::from(fence_wait_ms)),
                ("source_b", Value::from(src.b as i64)),
                ("source_g", Value::from(src.g as i64)),
                ("source_r", Value::from(src.r as i64)),
                ("source_a", Value::from(src.a as i64)),
                ("source_x", Value::from(src.x as i64)),
                ("source_y", Value::from(src.y as i64)),
                ("source_width", Value::from(src.width as i64)),
                ("source_height", Value::from(src.height as i64)),
                ("source_format", Value::from(src.format as i64)),
                ("backbuffer_b", Value::from(dst.b as i64)),
                ("backbuffer_g", Value::from(dst.g as i64)),
                ("backbuffer_r", Value::from(dst.r as i64)),
                ("backbuffer_a", Value::from(dst.a as i64)),
                ("backbuffer_x", Value::from(dst.x as i64)),
                ("backbuffer_y", Value::from(dst.y as i64)),
                ("backbuffer_width", Value::from(dst.width as i64)),
                ("backbuffer_height", Value::from(dst.height as i64)),
                ("backbuffer_format", Value::from(dst.format as i64)),
            ],
        );
    }

    fn recreate_backbuffer(&mut self, present_initial_black: bool) -> Result<(), String> {
        let backbuffer: ID3D11Texture2D = unsafe {
            self.swap_chain
                .GetBuffer(0)
                .map_err(|e| format!("IDXGISwapChain::GetBuffer: {e:?}"))?
        };
        let mut backbuffer_view = None;
        unsafe {
            self.d3d_device1
                .CreateRenderTargetView(&backbuffer, None, Some(&mut backbuffer_view))
                .map_err(|e| format!("CreateRenderTargetView: {e:?}"))?;
        }
        let backbuffer_view: ID3D11RenderTargetView =
            backbuffer_view.ok_or_else(|| "CreateRenderTargetView returned null".to_string())?;
        unsafe {
            self.d3d_context
                .ClearRenderTargetView(&backbuffer_view, &[0.0, 0.0, 0.0, 1.0]);
            if present_initial_black {
                self.swap_chain
                    .Present(1, Default::default())
                    .ok()
                    .map_err(|e| format!("initial IDXGISwapChain::Present: {e:?}"))?;
            }
        }
        self.backbuffer = Some(backbuffer);
        Ok(())
    }
}

impl NativeBlackBackground {
    fn new(
        factory: &IDXGIFactory2,
        d3d_device: &ID3D11Device,
        d3d_device1: &ID3D11Device1,
        d3d_context: &ID3D11DeviceContext,
        dcomp_device: &IDCompositionDevice,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        let width = width.max(1);
        let height = height.max(1);
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: 0,
        };
        let swap_chain = unsafe {
            factory
                .CreateSwapChainForComposition(d3d_device, &desc, None::<&IDXGIOutput>)
                .map_err(|e| format!("CreateSwapChainForComposition background: {e:?}"))?
        };
        let visual = unsafe {
            let visual = dcomp_device
                .CreateVisual()
                .map_err(|e| format!("CreateVisual background: {e:?}"))?;
            visual
                .SetContent(&swap_chain)
                .map_err(|e| format!("IDCompositionVisual::SetContent background: {e:?}"))?;
            visual
        };
        let mut this = Self {
            swap_chain,
            _visual: visual,
            backbuffer: None,
            render_target: None,
            width,
            height,
        };
        this.recreate_backbuffer(d3d_device1, d3d_context)?;
        log_event(
            "background_init",
            &[
                ("width", Value::from(this.width as i64)),
                ("height", Value::from(this.height as i64)),
            ],
        );
        Ok(this)
    }

    fn resize(
        &mut self,
        d3d_device1: &ID3D11Device1,
        d3d_context: &ID3D11DeviceContext,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let width = width.max(1);
        let height = height.max(1);
        // T26 (Claude R3-6): backbuffer が None なら early-return しない。前回 `recreate_backbuffer`
        // が失敗した直後だと size マッチでも backbuffer=None で固着しており、ここを通さないと
        // half-dead 状態が永久に続く。ResizeBuffers は同 size を渡しても基本 no-op なので
        // 二重呼び出しでも安全。
        if self.width == width && self.height == height && self.backbuffer.is_some() {
            return Ok(());
        }
        self.render_target = None;
        self.backbuffer = None;
        unsafe {
            self.swap_chain
                .ResizeBuffers(
                    0,
                    width,
                    height,
                    DXGI_FORMAT_UNKNOWN,
                    DXGI_SWAP_CHAIN_FLAG(0),
                )
                .map_err(|e| format!("background IDXGISwapChain::ResizeBuffers: {e:?}"))?;
        }
        self.width = width;
        self.height = height;
        self.recreate_backbuffer(d3d_device1, d3d_context)?;
        Ok(())
    }

    fn recreate_backbuffer(
        &mut self,
        d3d_device1: &ID3D11Device1,
        d3d_context: &ID3D11DeviceContext,
    ) -> Result<(), String> {
        let backbuffer: ID3D11Texture2D = unsafe {
            self.swap_chain
                .GetBuffer(0)
                .map_err(|e| format!("background IDXGISwapChain::GetBuffer: {e:?}"))?
        };
        let mut render_target = None;
        unsafe {
            d3d_device1
                .CreateRenderTargetView(&backbuffer, None, Some(&mut render_target))
                .map_err(|e| format!("background CreateRenderTargetView: {e:?}"))?;
        }
        let render_target: ID3D11RenderTargetView = render_target
            .ok_or_else(|| "background CreateRenderTargetView returned null".to_string())?;
        unsafe {
            d3d_context.ClearRenderTargetView(&render_target, &[0.0, 0.0, 0.0, 1.0]);
            self.swap_chain
                .Present(1, Default::default())
                .ok()
                .map_err(|e| format!("background IDXGISwapChain::Present: {e:?}"))?;
        }
        self.backbuffer = Some(backbuffer);
        self.render_target = Some(render_target);
        Ok(())
    }
}

impl NativeEguiOverlay {
    fn new(
        visual: IDCompositionVisual,
        dcomp_device: &IDCompositionDevice,
        root_visual: &IDCompositionVisual,
        after_visual: Option<&IDCompositionVisual>,
        dcomp_target: NativeRenderTarget,
        width: u32,
        height: u32,
        os_pixels_per_point: f32,
        window_observation: NativeWindowObservation,
        _cursor_hide_delay_secs: f32,
        ui_scale: f32,
        text_contrast: crate::settings::TextContrast,
        ui_font: crate::settings::UiFontSettings,
        health: Arc<crate::video::native_window_health::NativeWindowHealth>,
        window_epoch: u64,
    ) -> Result<Self, String> {
        // 本関数は placement 切替 (F12 の main ⇄ 別ウィンドウ) のたびに丸ごと走る。
        // Surface / Renderer / Context は窓ごとに作るが、Instance と compatible な
        // DeviceEpoch は process-owned service から再利用する (backlog §1.122)。
        let overlay_t0 = Instant::now();
        let service_t0 = Instant::now();
        let gpu_service = overlay_gpu_service();
        let instance_ms = service_t0.elapsed().as_secs_f64() * 1000.0;
        let surface_t0 = Instant::now();
        let surface =
            gpu_service.create_composition_surface(visual.as_raw() as *mut core::ffi::c_void)?;
        let surface_ms = surface_t0.elapsed().as_secs_f64() * 1000.0;
        let epoch_selection = gpu_service.select_or_create_epoch(&surface)?;
        let adapter_ms = epoch_selection.adapter_ms;
        let device_ms = epoch_selection.device_ms;
        let device_reused = epoch_selection.reused;
        let gpu_epoch = epoch_selection.epoch;
        let device_generation = gpu_epoch.generation();
        let caps = surface.get_capabilities(gpu_epoch.adapter());
        let format = choose_overlay_surface_format(&caps.formats)?;
        let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::AutoVsync) {
            wgpu::PresentMode::AutoVsync
        } else {
            *caps
                .present_modes
                .first()
                .ok_or_else(|| "wgpu DComp overlay surface has no present modes".to_string())?
        };
        let alpha_mode = if caps
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
        {
            wgpu::CompositeAlphaMode::PreMultiplied
        } else {
            wgpu::CompositeAlphaMode::Auto
        };
        let renderer_t0 = Instant::now();
        let renderer = egui_wgpu::Renderer::new(
            gpu_epoch.device(),
            format,
            egui_wgpu::RendererOptions::default(),
        );
        let renderer_ms = renderer_t0.elapsed().as_secs_f64() * 1000.0;
        let ctx_t0 = Instant::now();
        let egui_ctx = egui::Context::default();
        crate::ime_focus::install_ime_input_policy(&egui_ctx);
        crate::egui_focus_policy::install_tab_shortcut_focus_policy(&egui_ctx);
        crate::double_click_time::configure_context(&egui_ctx);
        egui_ctx.options_mut(|options| options.zoom_with_keyboard = false);
        let fonts_t0 = Instant::now();
        configure_overlay_fonts(&egui_ctx, &ui_font);
        let fonts_ms = fonts_t0.elapsed().as_secs_f64() * 1000.0;
        configure_overlay_style(&egui_ctx, text_contrast);
        let ctx_ms = ctx_t0.elapsed().as_secs_f64() * 1000.0;
        let ui_scale = crate::settings::normalize_ui_scale_factor(ui_scale);
        let pixels_per_point = effective_overlay_pixels_per_point(os_pixels_per_point, ui_scale);
        let this = Self {
            health,
            window_epoch,
            surface,
            visual,
            dcomp_device: dcomp_device.clone(),
            root_visual: root_visual.clone(),
            after_visual: after_visual.cloned(),
            gpu_epoch,
            format,
            present_mode,
            alpha_mode,
            renderer,
            egui_ctx,
            _dcomp_target_lease: dcomp_target,
            window_observation,
            last_text_input_focus_claim_at: None,
            started_at: Instant::now(),
            pending_events: Vec::new(),
            native_touch: crate::video::native_touch::NativeTouchAdapter::default(),
            panorama_pose: None,
            modifiers: egui::Modifiers::default(),
            pointer_pos: None,
            event_count: 0,
            render_count: 0,
            dirty: true,
            next_repaint_deadline: None,
            wants_pointer_input: false,
            wants_keyboard_input: false,
            video_position_secs: 0.0,
            video_duration_secs: 0.0,
            video_is_playing: false,
            video_volume: 1.0,
            video_muted: false,
            video_limiter_ceiling_hit_seq: 0,
            video_limiter_visible_until: None,
            video_playback_speed: 1.0,
            video_frame_step_active: false,
            video_is_seeking: false,
            video_seek_serial: 0,
            seek_status_started_at: None,
            seek_status_visible_since: None,
            seek_status_visible: false,
            video_speed_popup_open: false,
            frame_step_hold: None,
            video_loop_enabled: false,
            video_loop_mode: crate::settings::VideoLoopMode::Off,
            video_continuous_mode: crate::video::VideoContinuousMode::Off,
            video_checked: false,
            vst3_available: false,
            hud_dimmed: false,
            audio_only: false,
            vst3_panel: None,
            first_frame_presented: false,
            video_error: None,
            preparing_status: crate::video::avio_progress::PreparingStatus {
                phase: crate::video::avio_progress::prep_phase::OPENING,
                bytes_read: 0,
                file_size: 0,
            },
            toast: None,
            perf_visible: false,
            perf_history: VecDeque::with_capacity(256),
            perf_latest: NativeOverlayPerfSnapshot::default(),
            perf_last_dirty: Instant::now(),
            perf_pause_gap_pending: false,
            last_seek_target_secs: None,
            last_thumbnail_request_secs: None,
            last_thumbnail_request_at: None,
            hover_preview_target_secs: None,
            hover_preview_anchor_x: None,
            hover_preview_pinned: false,
            last_drawn_preview_rect: None,
            last_drawn_vst3_panel_rect: None,
            last_emitted_vst3_panel_pos: None,
            last_drawn_toast_rect: None,
            last_drawn_speed_popup_rect: None,
            last_drawn_bookmark_editor_rect: None,
            last_drawn_bulk_bookmark_dialog_rect: None,
            last_drawn_shortcut_help_rect: None,
            last_drawn_ring_picker_rect: None,
            last_drawn_ring_guide_rect: None,
            hover_thumbnail: None,
            hover_texture: None,
            hover_texture_key: None,
            timeline_markers: Vec::new(),
            jump_entries: Vec::new(),
            video_adjustments: crate::creative_lut::VideoAdjustments::default(),
            video_scale_filter: VideoScaleFilter::OsDefault,
            video_downscale_smoothing_percent: 0,
            video_scale_outside_note: None,
            video_anime4k_budget: crate::video::anime4k_policy::VideoAnime4kBudgetPreset::default(),
            video_anime4k_status: NativeVideoAnime4kStatus::Waiting,
            video_preset_slots: vec![None; 10].into(),
            creative_lut_choices: Arc::from([]),
            video_left_panel_tab: NativeVideoLeftPanelTab::Jump,
            video_adjustment_tab: NativeVideoAdjustmentTab::ColorTone,
            bookmark_title_edit: None,
            bulk_bookmark_dialog: None,
            shortcut_help_open: false,
            tag_picker_open: false,
            tag_picker_input: String::new(),
            tag_picker_focus_request: false,
            tag_picker_recent_tab: false,
            tag_panel_sticky_item_key: None,
            tag_panel_sticky_tags: Vec::new(),
            video_metadata: None,
            fallback_file_name: String::new(),
            navigation_preview: None,
            navigation_preview_texture: None,
            tile_overlay: None,
            seek_strip: None,
            seek_strip_material_availability:
                crate::video::seek_strip::SeekStripMaterialAvailability::Unknown,
            ring_picker_overlay: None,
            ring_guide_overlay: None,
            tile_textures: HashMap::new(),
            seek_strip_textures: HashMap::new(),
            seek_strip_wave_texture: None,
            seek_row_gesture: None,
            seek_strip_drag_origin: None,
            last_seek_strip_window_request: None,
            jump_textures: HashMap::new(),
            top_bar_drawn_visible: false,
            bottom_hud_visible: false,
            top_bar_visible: false,
            right_panel_visible: false,
            jump_panel_visible: false,
            top_bar_locked: false,
            bottom_lock: VideoBottomLock::None,
            side_panel_mode: FsSidePanelMode::Hover,
            info_panel_open: crate::ui_helpers::MetadataPanelOpenState::Closed,
            left_panel_open: crate::ui_helpers::MetadataPanelOpenState::Closed,
            right_panel_hover_latched: false,
            jump_panel_hover_latched: false,
            side_panel_escape_consumed: false,
            external_drag_in_progress: false,
            raw_hover_pos: None,
            pending_overlay_commands: Vec::new(),
            last_volume_target: None,
            visual_attached: false,
            ui_scale,
            pixels_per_point,
            width: width.max(1),
            height: height.max(1),
            normalize_state: crate::video::normalize_types::NormalizeOverlayState::default(),
        };
        let configure_t0 = Instant::now();
        this.configure()?;
        let configure_ms = configure_t0.elapsed().as_secs_f64() * 1000.0;
        crate::logger::log(format!(
            "native-presenter: egui overlay created in {:.1}ms \
             (wgpu_instance={instance_ms:.1} surface={surface_ms:.1} \
             adapter={adapter_ms:.1} device={device_ms:.1} reused={device_reused} \
             generation={device_generation} renderer={renderer_ms:.1} \
             egui_ctx={ctx_ms:.1} of_which_fonts={fonts_ms:.1} configure={configure_ms:.1}) \
             epoch={window_epoch}",
            overlay_t0.elapsed().as_secs_f64() * 1000.0,
        ));
        log_event(
            "egui_overlay_init",
            &[
                ("width", Value::from(this.width as i64)),
                ("height", Value::from(this.height as i64)),
                ("format", Value::from(format!("{:?}", this.format))),
                (
                    "present_mode",
                    Value::from(format!("{:?}", this.present_mode)),
                ),
                ("alpha_mode", Value::from(format!("{:?}", this.alpha_mode))),
                (
                    "adapter",
                    Value::from(this.gpu_epoch.adapter().get_info().name),
                ),
                ("device_generation", Value::from(device_generation)),
                ("device_reused", Value::from(device_reused)),
                ("configure_ms", Value::from(configure_ms)),
                ("pixels_per_point", Value::from(this.pixels_per_point)),
                ("visual_attached", Value::from(this.visual_attached)),
            ],
        );
        Ok(this)
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<Vec<NativeWindowIntent>, String> {
        let width = width.max(1);
        let height = height.max(1);
        if self.width == width && self.height == height {
            return Ok(Vec::new());
        }
        self.width = width;
        self.height = height;
        self.configure()?;
        self.dirty = true;
        self.render_once().map(|(_, intents)| intents)
    }

    /// CP8: DPI 変更を反映する。`pixels_per_point = os_ppp * ui_scale`。
    /// 戻り値: 値が変わったかどうか。変わった場合は呼び出し側で next render の
    /// region 再計算を期待する (= `dirty = true`)。
    fn set_os_pixels_per_point(&mut self, os_ppp: f32) -> bool {
        self.set_effective_pixels_per_point(effective_overlay_pixels_per_point(
            os_ppp,
            self.ui_scale,
        ))
    }

    fn set_effective_pixels_per_point(&mut self, ppp: f32) -> bool {
        let new_ppp = ppp.clamp(0.5, 16.0);
        if (self.pixels_per_point - new_ppp).abs() < f32::EPSILON {
            return false;
        }
        self.pixels_per_point = new_ppp;
        self.dirty = true;
        true
    }

    fn push_native_events(
        &mut self,
        events: &[crate::video::native_window::NativeVideoWindowEvent],
    ) {
        for event in events {
            self.push_native_event(event.clone());
        }
    }

    /// Builds the Phase 1 touch exclusion approximation.
    ///
    /// `compute_hud_regions()` returns the HUD HWND input-claim regions, not
    /// exact painted egui chrome response rects. In particular, its drag
    /// capture path may return the full surface while egui reports a pointer
    /// down. Native Cancel must therefore release synthetic primary-down state
    /// before `PointerGone`, or this approximation would remain capture-all.
    fn native_touch_geometry(&self) -> crate::touch_input::TapZoneGeometry {
        let ppp = self.pixels_per_point;
        let surface = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(self.width as f32 / ppp, self.height as f32 / ppp),
        );
        let excluded = self
            .compute_hud_regions()
            .into_iter()
            .map(|rect| {
                egui::Rect::from_min_max(
                    crate::video::native_touch::native_client_pixels_to_points(
                        [rect.left, rect.top],
                        ppp,
                    ),
                    crate::video::native_touch::native_client_pixels_to_points(
                        [rect.right, rect.bottom],
                        ppp,
                    ),
                )
            })
            .collect();
        crate::touch_input::TapZoneGeometry {
            surface,
            excluded,
            behavior: crate::touch_input::TouchSurfaceBehavior::Viewer {
                accepts_pinch: self.panorama_pose.is_some(),
                tap_zones: true,
            },
        }
    }

    fn push_native_touch_event(
        &mut self,
        touch: crate::video::native_window::NativeVideoTouchEvent,
    ) {
        let debug_window = match touch.source {
            crate::video::native_window::NativeVideoWindowSource::Presenter => {
                crate::touch_debug::TouchDebugWindow::Presenter
            }
            crate::video::native_window::NativeVideoWindowSource::Hud => {
                crate::touch_debug::TouchDebugWindow::Hud
            }
        };
        let geometry = self.native_touch_geometry();
        let now_ms = self.started_at.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let output =
            self.native_touch
                .handle_event(touch, &geometry, now_ms, self.pixels_per_point);
        crate::touch_debug::log_native_touch_coordinates(
            debug_window,
            touch.pointer_id,
            [touch.x, touch.y],
            output.pos,
        );
        self.enqueue_native_pointer_events(output.egui_events);
        let touch_commands = self.native_touch.take_commands();
        let dismiss_touch_panels = touch_commands.iter().copied().any(|command| {
            native_touch_panel_tap_command_dismisses_before_dispatch(
                command,
                self.left_panel_open,
                self.info_panel_open,
            )
        });
        for command in &touch_commands {
            crate::touch_debug::log_native_touch_command(debug_window, command);
        }
        if dismiss_touch_panels {
            let [close_left, close_right] =
                native_touch_owned_panel_sides(self.left_panel_open, self.info_panel_open);
            if close_left {
                self.left_panel_open = crate::ui_helpers::MetadataPanelOpenState::Closed;
            }
            if close_right {
                self.tag_picker_open = false;
                self.pending_overlay_commands
                    .push(NativeOverlayCommand::DismissTouchSidePanels);
            }
        } else {
            for command in touch_commands {
                match command {
                    crate::video::native_touch::NativeVideoTouchCommand::ToggleChrome => {
                        self.native_touch.toggle_chrome();
                    }
                    crate::video::native_touch::NativeVideoTouchCommand::SeekRelative {
                        delta_secs,
                    } => {
                        self.pending_overlay_commands
                            .push(NativeOverlayCommand::SeekRelative { delta_secs });
                    }
                    crate::video::native_touch::NativeVideoTouchCommand::PanoramaDrag {
                        delta_points,
                        viewport_height_points,
                    } => {
                        self.pending_overlay_commands
                            .push(NativeOverlayCommand::PanoramaDrag {
                                delta_points,
                                viewport_height_points,
                            });
                    }
                    crate::video::native_touch::NativeVideoTouchCommand::LearnAndShowChrome => {
                        self.native_touch.show_chrome();
                        self.pending_overlay_commands
                            .push(NativeOverlayCommand::TouchChromeLearned);
                    }
                }
            }
        }
        self.dirty = true;
    }

    fn enqueue_native_pointer_events(&mut self, events: Vec<egui::Event>) {
        for event in &events {
            match event {
                egui::Event::PointerMoved(pos) => self.pointer_pos = Some(*pos),
                egui::Event::PointerGone => self.pointer_pos = None,
                _ => {}
            }
        }
        self.pending_events.extend(events);
    }

    fn hud_dimmed_suppresses_overlay_pointer_event(
        event: &crate::video::native_window::NativeVideoWindowEvent,
    ) -> bool {
        use crate::video::native_window::NativeVideoWindowEvent as NativeEvent;

        matches!(
            event,
            NativeEvent::MouseMove(_)
                | NativeEvent::MouseButton(_)
                | NativeEvent::MouseWheel(_)
                | NativeEvent::MouseLeave
        )
    }

    fn clear_overlay_pointer_for_dimmed_hud(&mut self) {
        if self.pointer_pos.take().is_some() {
            self.pending_events.push(egui::Event::PointerGone);
            self.dirty = true;
        }
    }

    fn restore_active_hud_hover_after_undim(
        pointer_pos: &mut Option<egui::Pos2>,
        raw_hover_pos: &mut Option<egui::Pos2>,
    ) -> Option<egui::Pos2> {
        let restored = raw_hover_pos.take()?;
        *pointer_pos = Some(restored);
        Some(restored)
    }

    fn visibility_hover_pos(&self) -> Option<egui::Pos2> {
        if self.hud_dimmed {
            self.raw_hover_pos
        } else {
            self.pointer_pos
        }
    }

    fn native_hud_bottom_visible_from_hover(
        hover_pos: Option<egui::Pos2>,
        overlay_height_points: f32,
        external_drag_in_progress: bool,
        touch_chrome_latched: bool,
        locked: bool,
    ) -> bool {
        if locked {
            return true;
        }
        if touch_chrome_latched {
            return true;
        }
        if external_drag_in_progress {
            return false;
        }
        hover_pos.is_some_and(|pos| pos.y >= (overlay_height_points - 220.0).max(0.0))
    }

    fn native_hud_top_visible_from_hover(
        hover_pos: Option<egui::Pos2>,
        currently_visible: bool,
        external_drag_in_progress: bool,
        touch_chrome_latched: bool,
        locked: bool,
    ) -> bool {
        if locked {
            return true;
        }
        if touch_chrome_latched {
            return true;
        }
        if external_drag_in_progress {
            return false;
        }
        hover_pos.is_some_and(|pos| {
            let y_max = if currently_visible { 76.0 } else { 36.0 };
            pos.y <= y_max
        })
    }

    fn native_bar_visibility_snapshot(
        hud_visible: bool,
        top_bar_visible: bool,
        top_bar_hover_visible: bool,
        side_panel_visible: bool,
        tile_overlay_visible: bool,
        navigation_preview_visible: bool,
    ) -> NativeBarVisibilitySnapshot {
        let regular_chrome_visible = !tile_overlay_visible && !navigation_preview_visible;
        NativeBarVisibilitySnapshot {
            // 上部の実描画条件と同じ。固定 / hover の元判定ではなく、tile/nav と
            // side panel 連動まで通した最終値を region 用 snapshot にする。
            top_bar_visible: regular_chrome_visible && (top_bar_visible || side_panel_visible),
            // 上部固定は下部へ漏らさない。上端 hover と side panel による従来の
            // 連動表示だけを tile/nav ガードの内側で下部へ伝える。
            bottom_hud_visible: regular_chrome_visible
                && (hud_visible || side_panel_visible || top_bar_hover_visible),
        }
    }

    fn dimmed_hover_chrome_visible(&self) -> bool {
        if !self.hud_dimmed {
            return false;
        }
        self.hud_visible() || self.top_bar_visible()
    }

    fn update_raw_hover_pos_from_native_event(
        &mut self,
        event: &crate::video::native_window::NativeVideoWindowEvent,
    ) {
        use crate::video::native_window::NativeVideoWindowEvent as NativeEvent;

        self.raw_hover_pos = match event {
            NativeEvent::MouseMove(mouse) => Some(self.native_pos(mouse.x, mouse.y)),
            NativeEvent::MouseButton(button) => Some(self.native_pos(button.x, button.y)),
            NativeEvent::MouseWheel(wheel) => Some(self.native_pos(wheel.x, wheel.y)),
            NativeEvent::MouseLeave => None,
            _ => self.raw_hover_pos,
        };
    }

    fn push_native_event(&mut self, event: crate::video::native_window::NativeVideoWindowEvent) {
        use crate::video::native_window::{
            NativeVideoImeEvent, NativeVideoMouseButton, NativeVideoWindowEvent as NativeEvent,
        };

        self.event_count = self.event_count.saturating_add(1);
        if self.hud_dimmed && Self::hud_dimmed_suppresses_overlay_pointer_event(&event) {
            let was_visible = self.dimmed_hover_chrome_visible();
            self.update_raw_hover_pos_from_native_event(&event);
            self.clear_overlay_pointer_for_dimmed_hud();
            if self.dimmed_hover_chrome_visible() != was_visible {
                self.dirty = true;
            }
            return;
        }
        match event {
            NativeEvent::KeyDown(key) | NativeEvent::KeyUp(key) => {
                let modifiers = egui_modifiers(key.shift, key.ctrl, key.alt);
                self.modifiers = modifiers;
                if self.shortcut_help_open {
                    if matches!(event, NativeEvent::KeyDown(_))
                        && !key.repeat
                        && (key.virtual_key == 0x1B
                            || crate::keymap::native_video_context_shortcuts_help_key_down(&key))
                    {
                        self.shortcut_help_open = false;
                        self.dirty = true;
                    }
                    return;
                }
                if matches!(event, NativeEvent::KeyDown(_))
                    && crate::keymap::native_video_context_shortcuts_help_key_down(&key)
                    && self.can_open_shortcut_help()
                {
                    self.shortcut_help_open = true;
                    self.dirty = true;
                    return;
                }
                if matches!(event, NativeEvent::KeyDown(_))
                    && !key.repeat
                    && key.virtual_key == 0x1B
                    && !self.text_input_active()
                    && self.seek_strip.is_some()
                {
                    self.pending_overlay_commands
                        .push(NativeOverlayCommand::CloseSeekStrip {
                            cause: crate::video::seek_strip::SeekStripCloseCause::Escape,
                        });
                    self.side_panel_escape_consumed = true;
                    self.dirty = true;
                    return;
                }
                // ClickToShow の左右パネルは Escape で明示的に閉じる。App の通常 Escape
                // (fullscreen close) へ同じ key batch を転送しないよう consumed 印も立てる。
                if matches!(event, NativeEvent::KeyDown(_))
                    && !key.repeat
                    && key.virtual_key == 0x1B
                    && self.side_panel_mode.normalized() == FsSidePanelMode::ClickToShow
                    && !self.text_input_active()
                    && (self.left_panel_open.is_open() || self.info_panel_open.is_open())
                {
                    self.left_panel_open = crate::ui_helpers::MetadataPanelOpenState::Closed;
                    self.tag_picker_open = false;
                    if self.info_panel_open.is_open() {
                        self.pending_overlay_commands
                            .push(NativeOverlayCommand::ToggleClickInfoOpen);
                    }
                    self.side_panel_escape_consumed = true;
                    self.dirty = true;
                    return;
                }
                if !self.text_input_active() && native_video_fullscreen_shortcut_key(&key) {
                    return;
                }
                // text input active 時に Ctrl+V/C/X を OS clipboard と橋渡しする。
                // egui の TextEdit は Event::Paste / Copy / Cut だけ拾い、Ctrl 修飾の
                // raw Key event では発火しない (egui 0.33 の仕様)。Ctrl+A/Z/Y は Key
                // event のままで egui 側が処理する。
                //
                // 処理規約 (Codex C5/C14/C15 反映 2026-05-24):
                // - KeyDown / KeyUp の **両方** を suppress する。egui-winit と同様に
                //   Ctrl+V の raw Key event を egui に流さない (Down だけ抑えて Up を流すと
                //   release-without-press 状態になる)。
                // - IME 変換中 (`ime_composing()`) は intercept しない: 変換中に Ctrl+V を
                //   押した場合 Windows IME がそのキーを処理する余地を残す。
                //   commit 直後は通常通り Ctrl+V を取り込む (Codex P3)。
                // - クリップボードが空 / 非テキスト形式でも intercept は **成立とみなす**
                //   (= 'V' 文字を typing 扱いで挿入してしまうのを防ぐ)。
                // IME 関連の判定は **composition 中のみ** (= plugin state に pending
                // event を投影する `ime_composing()`) で
                // 行う。`ime_input_active()` の 300ms grace は Enter/Escape (確定キー
                // ハイジャック) 対策のもので、Ctrl+V/C/X に適用すると IME 確定直後の
                // 素早いペーストが落ちる (Codex P3 2026-05-24: composition 中だけ抑止し、
                // commit 後の通常ショートカットは通す)。
                let is_clipboard_shortcut = key.ctrl
                    && !key.alt
                    && self.text_input_active()
                    && !self.ime_composing()
                    && matches!(key.virtual_key, 0x43 | 0x56 | 0x58); // C/V/X
                if is_clipboard_shortcut {
                    if matches!(event, NativeEvent::KeyDown(_)) {
                        match key.virtual_key {
                            0x56 => {
                                // Ctrl+V: Windows の CF_UNICODETEXT は CRLF 改行なので
                                // \r\n / 単独 \r を \n に正規化する。クリップボード読み出しが
                                // 失敗 (空 / 非テキスト) しても intercept は成立 (raw 'V' 入力防止)。
                                if let Some(text) = read_clipboard_text_windows() {
                                    let normalized = normalize_clipboard_newlines(&text);
                                    if let Some(d) = self.bulk_bookmark_dialog.as_mut() {
                                        // BulkBookmarkDialog: focus 状態に依存しない直接代入
                                        // パスを使う (Codex C8 first-paste 取りこぼし防止)。
                                        // textarea は「リストを一気に貼る」想定なので、
                                        // カーソル位置挿入よりも末尾追記の方が UX が安定する。
                                        d.pending_paste = Some(normalized);
                                        d.request_focus = true;
                                        self.dirty = true;
                                    } else {
                                        // 名称編集ダイアログ (singleline) は focus 取得が確実
                                        // なので Event::Paste 経路のまま。
                                        self.pending_events.push(egui::Event::Paste(normalized));
                                        self.dirty = true;
                                    }
                                }
                            }
                            0x43 => {
                                self.pending_events.push(egui::Event::Copy);
                                self.dirty = true;
                            }
                            0x58 => {
                                self.pending_events.push(egui::Event::Cut);
                                self.dirty = true;
                            }
                            _ => unreachable!(),
                        }
                    }
                    // KeyDown / KeyUp 両方を return で抑止 (Codex C15)
                    return;
                }
                if let Some(egui_key) = egui_key_from_virtual_key(key.virtual_key) {
                    let pressed = matches!(event, NativeEvent::KeyDown(_));
                    self.pending_events.push(egui::Event::Key {
                        key: egui_key,
                        physical_key: Some(egui_key),
                        pressed,
                        repeat: key.repeat,
                        modifiers,
                    });
                    self.dirty = true;
                }
            }
            NativeEvent::Text(ch) => {
                if self.shortcut_help_open {
                    return;
                }
                self.pending_events.push(egui::Event::Text(ch.to_string()));
                self.dirty = true;
            }
            NativeEvent::Ime(ime) => {
                let ime = match ime {
                    NativeVideoImeEvent::Enabled => egui::ImeEvent::Enabled,
                    NativeVideoImeEvent::Preedit(text) => egui::ImeEvent::Preedit(text),
                    NativeVideoImeEvent::Commit(text) => egui::ImeEvent::Commit(text),
                    NativeVideoImeEvent::Disabled => egui::ImeEvent::Disabled,
                };
                self.pending_events.push(egui::Event::Ime(ime));
                self.dirty = true;
            }
            NativeEvent::MouseMove(mouse) => {
                let pos = self.native_pos(mouse.x, mouse.y);
                self.pointer_pos = Some(pos);
                self.modifiers = egui_modifiers(mouse.shift, mouse.ctrl, false);
                self.pending_events.push(egui::Event::PointerMoved(pos));
                self.dirty = true;
            }
            NativeEvent::MouseButton(button) => {
                let pos = self.native_pos(button.x, button.y);
                let modifiers = egui_modifiers(button.shift, button.ctrl, false);
                self.pointer_pos = Some(pos);
                self.modifiers = modifiers;
                self.pending_events.push(egui::Event::PointerMoved(pos));
                let egui_button = match button.button {
                    NativeVideoMouseButton::Left => egui::PointerButton::Primary,
                    NativeVideoMouseButton::Right => egui::PointerButton::Secondary,
                    NativeVideoMouseButton::Middle => egui::PointerButton::Middle,
                    NativeVideoMouseButton::Extra1 => egui::PointerButton::Extra1,
                    NativeVideoMouseButton::Extra2 => egui::PointerButton::Extra2,
                };
                self.pending_events.push(egui::Event::PointerButton {
                    pos,
                    button: egui_button,
                    pressed: button.down,
                    modifiers,
                });
                self.dirty = true;
            }
            NativeEvent::MouseWheel(wheel) => {
                let pos = self.native_pos(wheel.x, wheel.y);
                let modifiers = egui_modifiers(wheel.shift, wheel.ctrl, false);
                self.pointer_pos = Some(pos);
                self.modifiers = modifiers;
                // 端パネルの開閉ラッチは render_once 冒頭でしか更新されないため、同一イベント
                // バッチ内で端へ移動→即ホイールすると latch が stale のまま pointer_over_scroll_panel
                // が誤判定し、パネル上ホイールが前後アイテム切替に化ける (Codex P2)。ここで現在の
                // wheel 座標で latch を更新してから判定する。
                self.update_side_panel_hover_latches();
                let over_scroll_panel = self.pointer_over_scroll_panel(pos);
                // テキスト入力中央モーダル (一括ブックマーク登録ダイアログ / 名称編集) が
                // 出ているときは、ホイールで前後の動画に飛ばない (テキストや ScrollArea の
                // スクロールに使う、ユーザー報告 2026-05-24)。bookmark title 編集は単行で
                // スクロール不要だが、誤って動画切替されないようにこちらも対象に含める。
                let modal_dialog_visible = self.bulk_bookmark_dialog.is_some()
                    || self.bookmark_title_edit.is_some()
                    || self.shortcut_help_open;
                let pixels_per_point = self.pixels_per_point.max(f32::MIN_POSITIVE);
                let over_seek_strip = self.bottom_hud_visible
                    && self.seek_strip.is_some()
                    && self.tile_overlay.is_none()
                    && native_seek_strip_rect(
                        self.width as f32 / pixels_per_point,
                        self.height as f32 / pixels_per_point,
                    )
                    .contains(pos);
                let panorama_wheel = self.panorama_pose.is_some() && !self.audio_only;
                if panorama_wheel {
                    self.pending_overlay_commands
                        .push(NativeOverlayCommand::PanoramaWheel {
                            delta: i32::from(wheel.delta),
                        });
                } else if let Some(command) = immediate_native_wheel_command(
                    wheel.delta,
                    wheel.ctrl,
                    self.tile_overlay.is_some(),
                    over_seek_strip,
                    over_scroll_panel,
                    modal_dialog_visible,
                ) {
                    self.pending_overlay_commands.push(command);
                }
                self.pending_events.push(egui::Event::PointerMoved(pos));
                if !panorama_wheel {
                    self.pending_events.push(egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Line,
                        delta: egui::vec2(0.0, wheel.delta as f32 / 120.0),
                        modifiers,
                    });
                }
                self.dirty = true;
            }
            NativeEvent::MouseLeave => {
                if self.window_observation.has_hud && self.window_observation.cursor_input_owned {
                    return;
                }
                self.pointer_pos = None;
                self.pending_events.push(egui::Event::PointerGone);
                self.dirty = true;
            }
            NativeEvent::Touch(touch) => self.push_native_touch_event(touch),
            // viewer close は App 側に転送し、overlay には流さない。
            NativeEvent::CloseRequested { .. } => {}
            // 内部処理イベント (presenter thread が直接消費する)。overlay には流さない。
            NativeEvent::GeometryChanged { .. }
            | NativeEvent::DpiChanged { .. }
            | NativeEvent::RequestRaiseHud
            | NativeEvent::RequestFocusClaim
            | NativeEvent::CursorOwnership(_)
            | NativeEvent::Destroyed => {}
        }
    }

    fn update_video_state(
        &mut self,
        position_secs: f64,
        duration_secs: f64,
        is_playing: bool,
        volume: f64,
        muted: bool,
        limiter_ceiling_hit_seq: u64,
        playback_speed: f64,
        frame_step_active: bool,
        is_seeking: bool,
        seek_serial: u64,
    ) {
        let position_secs = finite_nonnegative(position_secs);
        let duration_secs = finite_nonnegative(duration_secs);
        let volume = finite_video_volume(volume);
        let playback_speed = crate::video::clock::clamp_playback_speed(playback_speed);
        let now = Instant::now();
        let duration_changed = (self.video_duration_secs - duration_secs).abs() > 0.001;
        let position_changed = (self.video_position_secs - position_secs).abs() >= 0.25;
        let playing_changed = self.video_is_playing != is_playing;
        let volume_changed = (self.video_volume - volume).abs() >= 0.005;
        let muted_changed = self.video_muted != muted;
        let limiter_changed = self.video_limiter_ceiling_hit_seq != limiter_ceiling_hit_seq;
        let speed_changed = (self.video_playback_speed - playback_speed).abs() >= 1.0e-6;
        let frame_step_changed = self.video_frame_step_active != frame_step_active;
        let seeking_changed = self.video_is_seeking != is_seeking;
        let seek_serial_changed = self.video_seek_serial != seek_serial;
        if !is_playing {
            self.perf_pause_gap_pending = true;
        }
        if seeking_changed || (is_seeking && seek_serial_changed) {
            if is_seeking {
                self.seek_status_started_at = Some(now);
            } else {
                self.seek_status_started_at = None;
            }
        }
        self.video_position_secs = position_secs;
        self.video_duration_secs = duration_secs;
        self.video_is_playing = is_playing;
        self.video_volume = volume;
        self.video_muted = muted;
        if limiter_changed {
            if limiter_ceiling_hit_seq > self.video_limiter_ceiling_hit_seq {
                self.video_limiter_visible_until = now.checked_add(LIMITER_INDICATOR_VISIBLE);
            } else {
                self.video_limiter_visible_until = None;
            }
        }
        self.video_limiter_ceiling_hit_seq = limiter_ceiling_hit_seq;
        self.video_playback_speed = playback_speed;
        self.video_frame_step_active = frame_step_active;
        self.video_is_seeking = is_seeking;
        self.video_seek_serial = seek_serial;
        if speed_changed {
            // 速度変更前のサンプルは旧 playback_speed のまま `perf_history` に残るが、
            // Y 軸スケールと gap 判定は最新サンプル群の median から導出されるため、
            // 過渡期に旧サンプルが新スケール基準で再解釈されて色がちらつく。新速度
            // で素のグラフから始めるためにクリアする。
            self.perf_history.clear();
            self.perf_pause_gap_pending = false;
        }
        if duration_changed
            || position_changed
            || playing_changed
            || volume_changed
            || muted_changed
            || limiter_changed
            || speed_changed
            || frame_step_changed
            || seeking_changed
            || seek_serial_changed
        {
            self.dirty = true;
        }
        self.schedule_seek_status_repaint(now);
        self.schedule_limiter_indicator_repaint(now);
    }

    fn native_pos(&self, x: i32, y: i32) -> egui::Pos2 {
        crate::video::native_touch::native_client_pixels_to_points([x, y], self.pixels_per_point)
    }

    fn set_perf_visible(&mut self, visible: bool) -> bool {
        if self.perf_visible == visible {
            return false;
        }
        self.perf_visible = visible;
        self.dirty = true;
        true
    }

    fn set_text_contrast(&mut self, contrast: crate::settings::TextContrast) {
        if crate::os_theme::current_text_contrast(&self.egui_ctx) == contrast {
            return;
        }
        configure_overlay_style(&self.egui_ctx, contrast);
        self.dirty = true;
    }

    fn push_perf_sample(
        &mut self,
        mut sample: NativeOverlayPerfSample,
        snapshot: NativeOverlayPerfSnapshot,
    ) {
        if let Some(prev) = self.perf_history.back() {
            // synthetic arrival は実時間ベースの間隔で前進させる必要がある。
            // 0.5x 再生では実 interval は source_delta_ms の 2 倍 = 横スクロールも
            // 2 倍遅くなるべき。speed-adjusted な effective interval を使う。
            let expected_ms = native_perf_expected_frame_ms_from_samples([*prev, sample])
                .unwrap_or_else(|| {
                    let sample_eff = native_perf_effective_interval_ms(&sample);
                    if sample_eff.is_finite() && sample_eff > 0.5 {
                        sample_eff
                    } else if prev.interval_ms.is_finite() && prev.interval_ms > 0.5 {
                        prev.interval_ms
                    } else {
                        16.67
                    }
                });
            sample.arrival =
                prev.arrival + Duration::from_secs_f32((expected_ms / 1000.0).clamp(0.001, 0.5));
            if self.perf_pause_gap_pending {
                sample.interval_ms = expected_ms;
            }
        }
        self.perf_pause_gap_pending = false;
        self.perf_latest = snapshot;
        self.perf_history.push_back(sample);
        while self.perf_history.len() > 1400 {
            self.perf_history.pop_front();
        }
        while self.perf_history.front().is_some_and(|front| {
            sample
                .arrival
                .saturating_duration_since(front.arrival)
                .as_secs_f32()
                > 6.5
        }) {
            self.perf_history.pop_front();
        }
        if self.perf_visible && self.perf_last_dirty.elapsed() >= Duration::from_millis(100) {
            self.perf_last_dirty = Instant::now();
            self.dirty = true;
        }
    }

    /// 動画ソース切替 (= `SwitchSource`) で呼ぶ。前ソースの perf 履歴がそのまま
    /// 残っていると、新ソースの fps / interval が混じった median で Y 軸が
    /// 算出され、新サンプルが溜まってきた頃に Y スケールがガクッと切り替わる
    /// (= ユーザー報告「動画切替後しばらくしてグラフ形状が突然変わる」)。
    /// `speed_changed` 経路と同じ理由でクリアする。
    fn reset_perf(&mut self) {
        self.perf_history.clear();
        self.perf_pause_gap_pending = false;
        self.perf_latest = NativeOverlayPerfSnapshot::default();
        if self.perf_visible {
            self.dirty = true;
        }
    }

    fn set_hover_thumbnail(&mut self, thumbnail: Option<NativeOverlayThumbnail>) {
        let new_key = thumbnail
            .as_ref()
            .map(|t| (t.width, t.height, thumbnail_rgba_key(t)));
        let old_key = self
            .hover_thumbnail
            .as_ref()
            .map(|t| (t.width, t.height, thumbnail_rgba_key(t)));
        if new_key == old_key {
            return;
        }
        self.hover_thumbnail = thumbnail;
        if self.hover_thumbnail.is_none() {
            self.hover_texture = None;
            self.hover_texture_key = None;
        }
        self.dirty = true;
    }

    fn set_hover_preview_pinned(&mut self, pinned: bool) {
        if self.hover_preview_pinned == pinned {
            return;
        }
        self.hover_preview_pinned = pinned;
        self.dirty = true;
    }

    fn set_timeline_markers(&mut self, markers: Vec<NativeOverlayTimelineMarker>) {
        if timeline_markers_match(&self.timeline_markers, &markers) {
            return;
        }
        self.timeline_markers = markers;
        self.dirty = true;
    }

    fn set_jump_entries(&mut self, entries: Vec<NativeOverlayJumpEntry>) {
        if jump_entries_match(&self.jump_entries, &entries) {
            return;
        }
        self.jump_entries = entries;
        self.dirty = true;
    }

    fn set_video_grade(&mut self, grade: &crate::creative_lut::VideoGradeSnapshot) {
        self.video_adjustments = grade.adjustments.clone();
        self.video_preset_slots = grade.slots.clone();
        self.creative_lut_choices = grade.choices.clone();
        self.dirty = true;
    }

    fn set_video_scale_state(
        &mut self,
        filter: VideoScaleFilter,
        smoothing_percent: u32,
        outside_note: Option<String>,
    ) {
        let smoothing_percent =
            crate::settings::sanitize_downscale_smoothing_percent(smoothing_percent);
        if self.video_scale_filter == filter
            && self.video_downscale_smoothing_percent == smoothing_percent
            && self.video_scale_outside_note == outside_note
        {
            return;
        }
        self.video_scale_filter = filter;
        self.video_downscale_smoothing_percent = smoothing_percent;
        self.video_scale_outside_note = outside_note;
        self.dirty = true;
    }

    fn set_panorama_pose(&mut self, panorama_pose: Option<PanoPose>) {
        let changed = self.panorama_pose != panorama_pose;
        self.panorama_pose = panorama_pose;
        let touch_changed = self
            .native_touch
            .configure_panorama(self.panorama_pose.is_some());
        self.dirty |= changed || touch_changed;
    }

    fn set_video_anime4k_state(
        &mut self,
        budget: crate::video::anime4k_policy::VideoAnime4kBudgetPreset,
        status: NativeVideoAnime4kStatus,
    ) {
        if self.video_anime4k_budget == budget && self.video_anime4k_status == status {
            return;
        }
        self.video_anime4k_budget = budget;
        self.video_anime4k_status = status;
        self.dirty = true;
    }

    fn set_metadata(&mut self, metadata: Option<NativeOverlayMetadata>) {
        let learned_snapshot = metadata
            .as_ref()
            .map(|metadata| metadata.touch_video_chrome_learned);
        let touch_changed = self
            .native_touch
            .configure_video_gestures(!self.audio_only, learned_snapshot);
        if self.video_metadata == metadata {
            self.dirty |= touch_changed;
            return;
        }
        self.video_metadata = metadata;
        self.dirty = true;
    }

    fn set_side_panel_state(
        &mut self,
        mode: FsSidePanelMode,
        info_panel_open: crate::ui_helpers::MetadataPanelOpenState,
    ) {
        let mode = mode.normalized();
        let mode_changed = self.side_panel_mode != mode;
        if !mode_changed && self.info_panel_open == info_panel_open {
            return;
        }
        self.side_panel_mode = mode;
        self.info_panel_open = info_panel_open;
        if mode_changed {
            self.left_panel_open = crate::ui_helpers::MetadataPanelOpenState::Closed;
            self.right_panel_hover_latched = false;
            self.jump_panel_hover_latched = false;
        }
        self.dirty = true;
    }

    fn set_bar_lock_state(&mut self, top_locked: bool, bottom_lock: VideoBottomLock) {
        if self.top_bar_locked == top_locked && self.bottom_lock == bottom_lock {
            return;
        }
        // touch chrome latch は入力セッションの状態として独立させる。固定表示への変更で
        // latch を開閉すると、固定解除後の中央タップ状態や左右ハンドルまで変わってしまう。
        self.top_bar_locked = top_locked;
        self.bottom_lock = bottom_lock;
        self.dirty = true;
    }

    fn reset_side_panel_session(&mut self) {
        let touch_reset = self.native_touch.reset_for_source_swap();
        let touch_changed = touch_reset.changed;
        self.enqueue_native_pointer_events(touch_reset.egui_events);
        let changed = self.left_panel_open.is_open()
            || self.right_panel_hover_latched
            || self.jump_panel_hover_latched
            || touch_changed;
        self.left_panel_open = crate::ui_helpers::MetadataPanelOpenState::Closed;
        self.right_panel_hover_latched = false;
        self.jump_panel_hover_latched = false;
        if changed {
            self.dirty = true;
        }
    }

    fn set_fallback_file_name(&mut self, file_name: String) {
        let file_name = if file_name.trim().is_empty() {
            String::new()
        } else {
            file_name
        };
        if self.fallback_file_name == file_name {
            return;
        }
        self.fallback_file_name = file_name;
        self.dirty = true;
    }

    fn set_normalize_state(&mut self, state: crate::video::normalize_types::NormalizeOverlayState) {
        if self.normalize_state == state {
            return;
        }
        self.normalize_state = state;
        self.dirty = true;
    }

    fn set_loop_enabled(&mut self, enabled: bool) {
        if self.video_loop_enabled == enabled {
            return;
        }
        self.video_loop_enabled = enabled;
        self.dirty = true;
    }

    fn set_loop_mode(&mut self, mode: crate::settings::VideoLoopMode) {
        if self.video_loop_mode == mode {
            return;
        }
        self.video_loop_mode = mode;
        self.dirty = true;
    }

    fn set_continuous_mode(&mut self, mode: crate::video::VideoContinuousMode) {
        if self.video_continuous_mode == mode {
            return;
        }
        self.video_continuous_mode = mode;
        self.dirty = true;
    }

    fn set_checked(&mut self, checked: bool) {
        if self.video_checked == checked {
            return;
        }
        self.video_checked = checked;
        self.dirty = true;
    }

    fn set_vst3_available(&mut self, available: bool) {
        if self.vst3_available == available {
            return;
        }
        self.vst3_available = available;
        if !available {
            self.vst3_panel = None;
            self.last_emitted_vst3_panel_pos = None;
        }
        self.dirty = true;
    }

    fn set_hud_dimmed(&mut self, dimmed: bool) {
        if self.hud_dimmed == dimmed {
            return;
        }
        self.hud_dimmed = dimmed;
        if !dimmed {
            // ParkedLive 中は操作を防ぐため egui の pointer_pos を消し、可視性専用の
            // raw_hover_pos だけを追跡する。active 復帰時はその実座標を通常 hover へ移管し、
            // activation click 後に次の MouseMove が来るまで HUD が消える隙間を作らない。
            // PointerButton は再送せず PointerMoved だけを渡すので、復帰クリックが HUD
            // ボタンの操作として二重発火することもない。
            if let Some(restored) = Self::restore_active_hud_hover_after_undim(
                &mut self.pointer_pos,
                &mut self.raw_hover_pos,
            ) {
                self.pending_events
                    .push(egui::Event::PointerMoved(restored));
            }
        }
        if dimmed {
            self.clear_overlay_pointer_for_dimmed_hud();
        }
        self.dirty = true;
    }

    fn set_audio_only(&mut self, audio_only: bool) {
        if self.audio_only == audio_only {
            return;
        }
        self.audio_only = audio_only;
        if audio_only {
            self.clear_seek_preview();
        }
        let learned_snapshot = self
            .video_metadata
            .as_ref()
            .map(|metadata| metadata.touch_video_chrome_learned);
        self.native_touch
            .configure_video_gestures(!audio_only, learned_snapshot);
        self.dirty = true;
    }

    fn set_vst3_panel(&mut self, panel: Option<NativeOverlayVst3Panel>) {
        if self.vst3_panel == panel {
            return;
        }
        let old_pos = self.vst3_panel.as_ref().and_then(|panel| panel.panel_pos);
        let new_pos = panel.as_ref().and_then(|panel| panel.panel_pos);
        if old_pos != new_pos {
            self.last_emitted_vst3_panel_pos = None;
        }
        self.vst3_panel = panel;
        self.dirty = true;
    }

    fn set_playback_status(
        &mut self,
        first_frame_presented: bool,
        error: Option<String>,
        prep_status: crate::video::avio_progress::PreparingStatus,
    ) {
        // first_frame_presented / error が変わらなくても、準備中フェーズの数値
        // (bytes_read など) は毎 tick 増えるので、`first_frame_presented = false`
        // の間は常に dirty を立てて再描画する。
        let prep_changed = !first_frame_presented
            && (self.preparing_status.phase != prep_status.phase
                || self.preparing_status.bytes_read != prep_status.bytes_read
                || self.preparing_status.file_size != prep_status.file_size);
        let other_changed =
            self.first_frame_presented != first_frame_presented || self.video_error != error;
        if !other_changed && !prep_changed {
            return;
        }
        self.first_frame_presented = first_frame_presented;
        self.video_error = error;
        self.preparing_status = prep_status;
        self.dirty = true;
    }

    fn seek_status_visible_at(&self, now: Instant) -> bool {
        seek_status_visible_for_times(
            self.video_is_seeking,
            self.seek_status_started_at,
            self.seek_status_visible_since,
            now,
        )
    }

    fn schedule_seek_status_repaint(&mut self, now: Instant) {
        let seek_delay_deadline = self
            .video_is_seeking
            .then(|| {
                self.seek_status_started_at
                    .and_then(|started| started.checked_add(SEEK_STATUS_DELAY))
                    .filter(|deadline| *deadline > now)
            })
            .flatten();
        let hold_deadline = self
            .seek_status_visible_since
            .and_then(|shown| shown.checked_add(SEEK_STATUS_MIN_VISIBLE))
            .filter(|deadline| *deadline > now);
        let deadline = match (seek_delay_deadline, hold_deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) | (None, Some(a)) => Some(a),
            (None, None) => None,
        };
        if let Some(deadline) = deadline {
            self.next_repaint_deadline = Some(
                self.next_repaint_deadline
                    .map_or(deadline, |current| current.min(deadline)),
            );
        }
    }

    fn limiter_indicator_visible_at(&mut self, now: Instant) -> bool {
        if let Some(until) = self.video_limiter_visible_until {
            if now < until {
                return true;
            }
            self.video_limiter_visible_until = None;
            self.dirty = true;
        }
        false
    }

    fn schedule_limiter_indicator_repaint(&mut self, now: Instant) {
        if let Some(deadline) = self
            .video_limiter_visible_until
            .filter(|deadline| *deadline > now)
        {
            self.next_repaint_deadline = Some(
                self.next_repaint_deadline
                    .map_or(deadline, |current| current.min(deadline)),
            );
        }
    }

    fn update_seek_status_for_render(&mut self, now: Instant) -> bool {
        let active_after_delay = self.video_is_seeking
            && self
                .seek_status_started_at
                .is_some_and(|started| now.duration_since(started) >= SEEK_STATUS_DELAY);
        let visible_hold_expired = self
            .seek_status_visible_since
            .is_some_and(|shown| now.duration_since(shown) >= SEEK_STATUS_MIN_VISIBLE);
        if !active_after_delay && visible_hold_expired {
            self.seek_status_visible_since = None;
        }
        if active_after_delay {
            // Keep this fresh while the slow seek is visible so completion gets the
            // full minimum hold instead of expiring from the first visible frame.
            self.seek_status_visible_since = Some(now);
        }
        let visible = self.seek_status_visible_at(now);
        self.seek_status_visible = visible;
        self.schedule_seek_status_repaint(now);
        visible
    }

    /// Cursor activity itself is reduced on the pump thread. Render only marks
    /// the overlay dirty so its current cursor policy is republished.
    fn mark_cursor_activity(&mut self) {
        self.dirty = true;
    }

    fn request_render(&mut self) {
        self.dirty = true;
    }

    fn show_toast(&mut self, text: String, centered: bool, linger: Option<Duration>) {
        if text.trim().is_empty() {
            return;
        }
        let linger =
            linger.unwrap_or_else(|| Duration::from_millis(if centered { 2500 } else { 1800 }));
        self.toast = Some(NativeOverlayToast {
            text,
            started_at: Instant::now(),
            centered,
            linger,
        });
        self.dirty = true;
    }

    fn set_tile_overlay(&mut self, tile_overlay: Option<NativeOverlayTileOverlay>) {
        if self.tile_overlay.is_none() && tile_overlay.is_none() {
            return;
        }
        if tile_overlay.is_none() {
            self.tile_textures.clear();
        } else {
            self.clear_seek_preview();
        }
        self.tile_overlay = tile_overlay;
        self.dirty = true;
    }

    fn set_seek_strip(
        &mut self,
        seek_strip: Option<NativeOverlaySeekStrip>,
        material_availability: crate::video::seek_strip::SeekStripMaterialAvailability,
    ) {
        let mode_changed = self.seek_strip.as_ref().map(|strip| strip.center.mode())
            != seek_strip.as_ref().map(|strip| strip.center.mode());
        if seek_strip.is_none() || (self.seek_strip.is_some() && mode_changed) {
            self.clear_seek_preview();
        }
        if self.seek_strip.is_none()
            && seek_strip.is_none()
            && self.seek_strip_material_availability == material_availability
        {
            return;
        }
        if seek_strip.is_none() {
            self.seek_strip_textures.clear();
            self.seek_strip_wave_texture = None;
            self.seek_strip_drag_origin = None;
            self.last_seek_strip_window_request = None;
        }
        self.seek_strip = seek_strip;
        self.seek_strip_material_availability = material_availability;
        self.dirty = true;
    }

    fn clear_seek_preview(&mut self) {
        let had_target = self.hover_preview_target_secs.take().is_some();
        self.hover_preview_anchor_x = None;
        let had_worker_request = self.last_thumbnail_request_secs.take().is_some();
        let had_request = had_target || had_worker_request;
        self.last_thumbnail_request_at = None;
        let had_drawn_rect = self.last_drawn_preview_rect.take().is_some();
        if had_request
            && !self
                .pending_overlay_commands
                .iter()
                .any(|command| matches!(command, NativeOverlayCommand::ClearSeekThumbnail))
        {
            self.pending_overlay_commands
                .push(NativeOverlayCommand::ClearSeekThumbnail);
        }
        if had_request || had_drawn_rect {
            self.dirty = true;
        }
    }

    fn reset_seek_preview_source_session(&mut self) {
        self.clear_seek_preview();
        let had_thumbnail = self.hover_thumbnail.take().is_some();
        let had_texture = self.hover_texture.take().is_some();
        let had_texture_key = self.hover_texture_key.take().is_some();
        let had_source_image = had_thumbnail || had_texture || had_texture_key;
        if had_source_image {
            self.dirty = true;
        }
    }

    fn set_ring_picker_overlay(&mut self, picker: Option<NativeOverlayRingPicker>) {
        if self.ring_picker_overlay == picker {
            return;
        }
        self.ring_picker_overlay = picker;
        self.last_drawn_ring_picker_rect = None;
        self.dirty = true;
    }

    fn set_ring_guide_overlay(&mut self, guide: Option<NativeOverlayRingGuide>) {
        if self.ring_guide_overlay == guide {
            return;
        }
        self.ring_guide_overlay = guide;
        self.last_drawn_ring_guide_rect = None;
        self.dirty = true;
    }

    fn set_navigation_preview(&mut self, preview: Option<NativeOverlayNavigationPreview>) {
        if self.navigation_preview.is_none() && preview.is_none() {
            return;
        }
        if preview.is_none() {
            self.navigation_preview_texture = None;
        } else {
            self.clear_seek_preview();
        }
        self.navigation_preview = preview;
        self.dirty = true;
    }

    fn sync_hover_thumbnail_texture(&mut self) {
        let Some(thumbnail) = self.hover_thumbnail.as_ref() else {
            self.hover_texture = None;
            self.hover_texture_key = None;
            return;
        };
        let key = (
            thumbnail.width,
            thumbnail.height,
            thumbnail_rgba_key(thumbnail),
        );
        if self.hover_texture_key == Some(key) {
            return;
        }
        let size = [thumbnail.width as usize, thumbnail.height as usize];
        if size[0] == 0 || size[1] == 0 || thumbnail.rgba.len() != size[0] * size[1] * 4 {
            return;
        }
        let color_image = egui::ColorImage::from_rgba_unmultiplied(size, thumbnail.rgba.as_ref());
        let texture = self.egui_ctx.load_texture(
            "native_seek_hover_thumbnail",
            color_image,
            egui::TextureOptions::LINEAR,
        );
        self.hover_texture = Some(texture);
        self.hover_texture_key = Some(key);
    }

    fn sync_navigation_preview_texture(&mut self) {
        let Some(preview) = self.navigation_preview.as_ref() else {
            self.navigation_preview_texture = None;
            return;
        };
        let Some(thumbnail) = preview.thumbnail.as_ref() else {
            self.navigation_preview_texture = None;
            return;
        };
        let key = (Arc::as_ptr(&thumbnail.rgba) as usize as u64)
            ^ (thumbnail.rgba.len() as u64)
            ^ ((thumbnail.width as u64) << 32)
            ^ thumbnail.height as u64
            ^ thumbnail.target_secs.to_bits();
        if self
            .navigation_preview_texture
            .as_ref()
            .is_some_and(|(cached_key, _)| *cached_key == key)
        {
            return;
        }
        let size = [thumbnail.width as usize, thumbnail.height as usize];
        if size[0] == 0 || size[1] == 0 || thumbnail.rgba.len() != size[0] * size[1] * 4 {
            self.navigation_preview_texture = None;
            return;
        }
        let color_image = egui::ColorImage::from_rgba_unmultiplied(size, thumbnail.rgba.as_ref());
        let texture = self.egui_ctx.load_texture(
            "native_video_navigation_preview",
            color_image,
            egui::TextureOptions::LINEAR,
        );
        self.navigation_preview_texture = Some((key, texture));
    }

    fn sync_tile_overlay_textures(&mut self) {
        let Some(tile_overlay) = self.tile_overlay.as_ref() else {
            self.tile_textures.clear();
            return;
        };
        self.tile_textures
            .retain(|idx, _| tile_overlay.tiles.get(*idx).is_some_and(Option::is_some));
        for (idx, slot) in tile_overlay.tiles.iter().enumerate() {
            let Some(tile) = slot.as_ref() else {
                self.tile_textures.remove(&idx);
                continue;
            };
            let key = tile.target_secs.to_bits() ^ ((tile.width as u64) << 32) ^ tile.height as u64;
            if self
                .tile_textures
                .get(&idx)
                .is_some_and(|(cached_key, _)| *cached_key == key)
            {
                continue;
            }
            let size = [tile.width as usize, tile.height as usize];
            if size[0] == 0 || size[1] == 0 || tile.rgba.len() != size[0] * size[1] * 4 {
                continue;
            }
            let color_image = egui::ColorImage::from_rgba_unmultiplied(size, tile.rgba.as_ref());
            let texture = self.egui_ctx.load_texture(
                format!("native_video_tile:{idx}"),
                color_image,
                egui::TextureOptions::LINEAR,
            );
            self.tile_textures.insert(idx, (key, texture));
        }
    }

    fn sync_seek_strip_textures(&mut self) {
        let Some(strip) = self.seek_strip.as_ref() else {
            self.seek_strip_textures.clear();
            self.seek_strip_wave_texture = None;
            return;
        };
        if matches!(
            strip.center,
            crate::video::seek_strip::SeekStripCenter::Waveform { .. }
        ) {
            self.seek_strip_textures.clear();
            let Some(wave) = strip.wave_image.as_ref() else {
                self.seek_strip_wave_texture = None;
                return;
            };
            let key = wave.window_start_secs.to_bits()
                ^ wave.window_end_secs.to_bits().rotate_left(11)
                ^ wave.bin_secs.to_bits().rotate_left(23)
                ^ ((wave.width as u64) << 32)
                ^ wave.height as u64;
            if self
                .seek_strip_wave_texture
                .as_ref()
                .is_some_and(|(cached_key, _)| *cached_key == key)
            {
                return;
            }
            let size = [wave.width as usize, wave.height as usize];
            if size[0] == 0 || size[1] == 0 || wave.rgba.len() != size[0] * size[1] * 4 {
                self.seek_strip_wave_texture = None;
                return;
            }
            let image = egui::ColorImage::from_rgba_unmultiplied(size, wave.rgba.as_ref());
            let texture = self.egui_ctx.load_texture(
                "native_video_seek_strip:wave",
                image,
                egui::TextureOptions::LINEAR,
            );
            self.seek_strip_wave_texture = Some((key, texture));
            return;
        }
        self.seek_strip_wave_texture = None;
        self.seek_strip_textures.retain(|index, _| {
            strip.cells.iter().any(|cell| {
                cell.index == *index
                    && matches!(&cell.content, NativeOverlaySeekStripCellContent::Ready(_))
            })
        });
        for cell in &strip.cells {
            let NativeOverlaySeekStripCellContent::Ready(thumbnail) = &cell.content else {
                self.seek_strip_textures.remove(&cell.index);
                continue;
            };
            let key = thumbnail.target_secs.to_bits()
                ^ ((thumbnail.width as u64) << 32)
                ^ thumbnail.height as u64;
            if self
                .seek_strip_textures
                .get(&cell.index)
                .is_some_and(|(cached_key, _)| *cached_key == key)
            {
                continue;
            }
            let size = [thumbnail.width as usize, thumbnail.height as usize];
            if size[0] == 0 || size[1] == 0 || thumbnail.rgba.len() != size[0] * size[1] * 4 {
                continue;
            }
            let image = egui::ColorImage::from_rgba_unmultiplied(size, thumbnail.rgba.as_ref());
            let texture = self.egui_ctx.load_texture(
                format!("native_video_seek_strip:{}", cell.index),
                image,
                egui::TextureOptions::LINEAR,
            );
            self.seek_strip_textures.insert(cell.index, (key, texture));
        }
    }

    fn sync_jump_entry_textures(&mut self) {
        self.jump_textures
            .retain(|idx, _| self.jump_entries.get(*idx).is_some());
        for (idx, entry) in self.jump_entries.iter().enumerate() {
            let Some(thumbnail) = entry.thumbnail.as_ref() else {
                continue;
            };
            let key = thumbnail_rgba_key(thumbnail)
                ^ ((thumbnail.width as u64) << 32)
                ^ thumbnail.height as u64;
            if self
                .jump_textures
                .get(&idx)
                .is_some_and(|(cached_key, _)| *cached_key == key)
            {
                continue;
            }
            let size = [thumbnail.width as usize, thumbnail.height as usize];
            if size[0] == 0 || size[1] == 0 || thumbnail.rgba.len() != size[0] * size[1] * 4 {
                continue;
            }
            let color_image =
                egui::ColorImage::from_rgba_unmultiplied(size, thumbnail.rgba.as_ref());
            let texture = self.egui_ctx.load_texture(
                format!("native_video_jump:{idx}"),
                color_image,
                egui::TextureOptions::LINEAR,
            );
            self.jump_textures.insert(idx, (key, texture));
        }
    }

    fn wants_periodic_tick(&self) -> bool {
        self.hud_visible()
            || self.jump_panel_visible()
            || self.top_bar_visible()
            || self.right_panel_visible()
            || self.vst3_panel_visible()
            || self.perf_visible
            || self.navigation_preview.is_some()
            || self.tile_overlay.is_some()
            || self.seek_strip.is_some()
            || self.hover_preview_target_secs.is_some()
            || self.frame_step_hold.is_some()
            || self.bookmark_title_edit.is_some()
            || self.bulk_bookmark_dialog.is_some()
            || self.shortcut_help_open
            || self.toast.is_some()
            || self.video_error.is_some()
            || !self.first_frame_presented
            || self.video_is_seeking
            || self.seek_status_visible_since.is_some()
            || self.video_limiter_visible_until.is_some()
    }

    fn repaint_due(&self, now: Instant) -> bool {
        self.next_repaint_deadline
            .is_some_and(|deadline| now >= deadline)
    }

    fn has_pointer_pos(&self) -> bool {
        self.pointer_pos.is_some()
    }

    fn needs_synthetic_pointer_move(&self, x: i32, y: i32) -> bool {
        let pos = self.native_pos(x, y);
        self.pointer_pos
            .is_none_or(|prev| prev.distance_sq(pos) > 0.25)
    }

    fn hover_tooltip_repaint_needed(&self) -> bool {
        self.pointer_pos.is_some()
            && (self.hud_visible()
                || self.top_bar_visible()
                || self.right_panel_visible()
                || self.jump_panel_visible()
                || self.vst3_panel_visible()
                || self.video_speed_popup_open
                || self.hover_preview_target_secs.is_some())
    }

    fn needs_render(&self) -> bool {
        self.dirty || !self.pending_events.is_empty()
    }

    fn render_if_dirty(&mut self) -> Result<NativeOverlayInputOutcome, String> {
        let hover_tooltip_repaint_needed = self.hover_tooltip_repaint_needed();
        if !self.dirty && self.pending_events.is_empty() && !hover_tooltip_repaint_needed {
            // CP5: dirty フラグ無しでも HUD regions は最新の visibility に合わせて
            // 出しておく (= 例えば mouse hover で bar が消えても、その後の VST z-order
            // 変更で再 raise されたとき HUD HWND の region がきちんと「穴」になっている
            // 必要があるため)。dirty 無しの場合は render はしないが regions だけ更新。
            return Ok(NativeOverlayInputOutcome {
                routing: self.input_routing(),
                commands: Vec::new(),
                window_intents: Vec::new(),
                hud_regions: self.compute_hud_regions(),
            });
        }
        // egui tooltip は hover 後に `request_repaint_after(tooltip_delay)` で開く。
        // native overlay には eframe の repaint callback が無いため、hover UI 表示中は
        // periodic tick で egui pass も回して delay 到達を拾う。
        // **再生停止バグ修正 (2026-06-19)**: タグ確定 Enter で `render_once()` が pending
        // イベントを overlay egui へ流すと、タグ付与後にピッカーが閉じて `text_input_active`
        // が false に落ちる。その「閉じた当の Enter」を `should_forward_to_ui` が転送扱いに
        // すると、App 側 (`handle_native_video_key_event`) で再生 toggle が走り動画が一時停止する
        // (IME 確定後の Enter で再現)。イベント処理 *前* のテキスト入力状態を捕まえ、転送判定は
        // これを OR して「受領時にテキスト入力中だったキーは App へ転送しない」を保証する。
        let text_input_active_before_events = self.text_input_active();
        let side_panel_escape_consumed = std::mem::take(&mut self.side_panel_escape_consumed);
        let (commands, window_intents) = self.render_once()?;
        // overlay が wheel を navigation / tile columns / strip range command に変換したフレームでは、
        // 同じ raw wheel イベントを App へ二重転送しないよう routing に印を付ける。
        let consumed_wheel = commands.iter().any(|c| {
            matches!(
                c,
                NativeOverlayCommand::NavigateItem { .. }
                    | NativeOverlayCommand::TileColumnsDelta { .. }
                    | NativeOverlayCommand::StepSeekStripRange { .. }
                    | NativeOverlayCommand::PanoramaWheel { .. }
            )
        });
        let mut routing = self.input_routing();
        routing.text_input_active |= text_input_active_before_events;
        routing.consumed_wheel = consumed_wheel;
        routing.modal_dialog_active |= side_panel_escape_consumed;
        // モーダル中央テキストダイアログの表示中は、App へ raw event を流さない
        // (Codex C1/C2/C3: dark backdrop 上の wheel/right-click が暴発する事故防止)。
        routing.modal_dialog_active |= self.modal_dialog_active_for_routing();
        Ok(NativeOverlayInputOutcome {
            routing,
            commands,
            window_intents,
            hud_regions: self.compute_hud_regions(),
        })
    }

    fn modal_dialog_active_for_routing(&self) -> bool {
        self.bulk_bookmark_dialog.is_some()
            || self.bookmark_title_edit.is_some()
            || self.shortcut_help_open
    }

    /// CP5: 現在表示中の overlay UI 要素の物理ピクセル RECT を集める。
    /// `apply_hud_regions(&regions)` で `SetWindowRgn` に渡されて、HUD HWND の
    /// 物理形状が更新される。
    ///
    /// **含めるもの (= 表示される UI rect。クリック可能でない受動インジケータも含む)**:
    /// - 上 hover bar (`top_bar_visible()`): 画面上端 0..76pt 帯
    /// - 下 HUD (`hud_visible()`): 画面下端 (H-220)..H pt 帯
    /// - right panel (`right_panel_visible()`): 画面右端の panel rect
    /// - jump panel (`jump_panel_visible`): 画面下半分の jump rect
    /// - touch panel handles (touch chrome latch 中): 左右端の 48pt rect
    /// - VST3 panel (`vst3_panel_visible()`): center modal panel
    /// - speed popup (`video_speed_popup_open`): 画面下端の popup
    /// - bookmark title editor (`bookmark_title_edit.is_some()`): center modal
    /// - normalize progress / scan UI (`normalize_state` の各 phase)
    /// - tile overlay (`tile_overlay.is_some()`): 全画面 tile grid
    /// - seek hover thumbnail + pin/bookmark (`hover_thumbnail.is_some()`)
    /// - checkmark indicator (`video_checked`)
    ///
    /// **含めないもの**:
    /// - activation zone (= bar 非表示状態の hover 検出範囲) — VST のノブやメニューが
    ///   上下端に重なったとき入力を奪わないため (Codex 5 P1 #1)。hover 検出は CP6 の
    ///   presenter polling 経路で代替。
    ///
    /// **`capture_all` フラグ**: egui `pointer.any_down() && wants_pointer_input` が
    /// true のフレーム (= drag 中) は全画面 RECT に置換して drag 維持。
    ///
    /// CP5 段階では各 UI 要素の正確な egui `response.rect` を持っていないため、
    /// **概算 RECT** (= 既知の固定高さ帯) で実装する。CP7 で有効化したあとの実機検証で
    /// rect ずれが出たら egui レイアウト結果から rect を引いてくる方式に補修する。
    fn compute_hud_regions(&self) -> Vec<RECT> {
        let ppp = self.pixels_per_point;
        let width_px = self.width.max(1) as i32;
        let height_px = self.height.max(1) as i32;

        if self.native_touch.first_run_help_visible() {
            return vec![RECT {
                left: 0,
                top: 0,
                right: width_px,
                bottom: height_px,
            }];
        }

        // capture_all: drag 中はクリックを HUD 経由で受け取りたいので region を画面全体に。
        let pointer_down = self.egui_ctx.input(|i| i.pointer.any_down());
        if pointer_down && self.wants_pointer_input {
            return vec![RECT {
                left: 0,
                top: 0,
                right: width_px,
                bottom: height_px,
            }];
        }

        let mut regions: Vec<RECT> = Vec::new();
        let to_px = |pt: f32| -> i32 { (pt * ppp).round() as i32 };
        let width_points = (self.width as f32 / ppp).max(1.0);
        let height_points = (self.height as f32 / ppp).max(1.0);
        // Codex CP9 実機 P1 #3 反映: egui::Rect → physical RECT 変換 helper。
        // panel 概算値ではなく `overlay_draw::native_*_rect` を直接物理ピクセルに変換することで、
        // 実 UI rect と region が一致して境界振動を起こさない。
        let rect_to_px = |r: egui::Rect| -> RECT {
            let left = (r.min.x * ppp).round() as i32;
            let top = (r.min.y * ppp).round() as i32;
            let right = (r.max.x * ppp).round() as i32;
            let bottom = (r.max.y * ppp).round() as i32;
            RECT {
                left: left.max(0).min(width_px),
                top: top.max(0).min(height_px),
                right: right.max(0).min(width_px),
                bottom: bottom.max(0).min(height_px),
            }
        };

        // 描画側の visibility snapshot を使って region を組み立てる。
        // tile overlay 表示中は通常 HUD UI が非表示 (= tile grid モード) なので region も別系統。
        let tile_overlay_visible = self.tile_overlay.is_some();
        let navigation_preview_visible = self.navigation_preview.is_some();
        let top_bar_visible_flag = self.top_bar_drawn_visible;
        let right_panel_visible_flag = self.right_panel_visible();
        let jump_panel_visible_flag = self.jump_panel_visible();
        let (left_callout_visible, right_callout_visible) = self.side_panel_callout_visibility();
        let panel_chrome_visible = top_bar_visible_flag;
        let raw_seek_status_visible = self.seek_status_visible
            && !tile_overlay_visible
            && !navigation_preview_visible
            && self.first_frame_presented
            && self.video_error.is_none();
        let seek_status_visible = raw_seek_status_visible;
        let status_visible = !tile_overlay_visible
            && !navigation_preview_visible
            && (self.video_error.is_some() || !self.first_frame_presented || seek_status_visible);
        // **top_bar_visible_flag / bottom_hud_visible** は描画側 (render_once) と
        // 完全一致させる。ここで hover / 固定 / tile/nav / panel 連動を再計算すると、
        // 描画と HWND の hit-test region / z-order が食い違うため、直前に描画した
        // 同じ snapshot だけを通す。
        // CP5 旧版の不具合では、片側だけの判定により「描画されたバーがクリックを
        // 受けない」状態になった。逆に region だけ true なら透明部分が入力を奪う。
        let bottom_hud_visible = self.bottom_hud_visible;

        // 上 hover bar (= 実描画 54pt = overlay_draw:1546 と一致)。
        //
        // ## region サイズの選択 (Codex 2026-05-12 P1 反映)
        //
        // **実描画 rect (54pt) だけ**を region に入れる。**活性化 zone (76pt) は region に
        // 入れない** (= polling / presenter wndproc 経由で pointer_pos が更新される経路に任せる)。
        //
        // ### なぜ実描画だけか
        //
        // `SetWindowRgn` で region に入れた領域は **DComp 透過でも HWND が入力を取る**
        // (= Codex 指摘の核心)。旧版で 180pt 入れていたのは「pointer が region 外に出ると
        // WM_MOUSEMOVE が HUD wndproc に届かないので bar 表示が振動する」懸念だったが、
        // 実際は **presenter HWND の wndproc も WM_MOUSEMOVE を `NativeEguiOverlay` に
        // push している** (presenter HWND は fullscreen 全画面で region 全域)。region 外でも
        // egui の pointer_pos は更新され続け、`top_bar_visible()` の hover 判定
        // (`pos.y <= 76`) は維持される。
        //
        // 旧版で region 内に「描画されない 126pt 帯」(= 54pt〜180pt) を入れていたのは、
        // この 126pt 帯 = VST の上部ヘッダ・タイトルバーが押せない原因だった (= ユーザー
        // 報告「VST 上端をドラッグして戻せない」「top bar 表示中に VST のヘッダクリックが
        // 効かない」)。
        if top_bar_visible_flag {
            regions.push(RECT {
                left: 0,
                top: 0,
                right: width_px,
                bottom: to_px(HUD_TOP_HEIGHT).min(height_px),
            });
        }

        // 下 HUD (seek 行 + コントロール 行) 表示中。**実描画 HUD_BOTTOM_HEIGHT (= 64pt) 帯**
        // (= `fixed_pos(0, height - HUD_BOTTOM_HEIGHT)` + `set_min_size(W, HUD_BOTTOM_HEIGHT)` と一致、
        // 動画 HUD 2 段化リデザインで旧 46pt から拡張)。
        //
        // ## region サイズの選択 (Codex 2026-05-12 P1 反映)
        //
        // 旧版は 220pt = `hud_visible()` の活性化 zone と同じだったが、これは hover 検出用
        // しきい値であって描画 zone ではない。220pt 入れていた 174pt 余分帯 (= 46pt〜220pt) は
        // VST のフッタ・下半分ボタン・キー操作領域が押せない原因だった (= ユーザー報告
        // 「下半分の VST ボタンが押せない」の主因)。
        // 活性化判定は presenter wndproc 経由の pointer_pos で維持されるので region は不要。
        if bottom_hud_visible {
            let bottom_band_top = (height_px - to_px(HUD_BOTTOM_HEIGHT)).max(0);
            regions.push(RECT {
                left: 0,
                top: bottom_band_top,
                right: width_px,
                bottom: height_px,
            });
        }
        if bottom_hud_visible && self.seek_strip.is_some() && !tile_overlay_visible {
            regions.push(rect_to_px(native_seek_strip_rect(
                width_points,
                height_points,
            )));
        }

        // Center status (error / preparing / slow seek): `draw_native_center_status`
        // と同じ box サイズを region に含め、HUD HWND の SetWindowRgn でクリップされないようにする。
        // T28: box サイズ計算は `native_center_status_rect` ヘルパーに集約 (重複式の防止)。
        if status_visible {
            let has_body = self.video_error.is_some();
            let title = if has_body {
                "動画を再生できません".to_owned()
            } else if !self.first_frame_presented {
                crate::video::avio_progress::build_preparing_message(self.preparing_status)
            } else {
                "シーク中...".to_owned()
            };
            let status_rect = super::overlay_draw::native_center_status_rect(
                width_points,
                height_points,
                &title,
                has_body,
            );
            regions.push(rect_to_px(status_rect));
        }

        // Right panel 表示中: `native_metadata_panel_rect` の実 rect を使う
        // (= 幅 430pt 上限、top=56pt から hover_bottom まで、Codex CP9 実機 P1 #3 反映)。
        if right_panel_visible_flag {
            regions.push(rect_to_px(super::overlay_draw::native_metadata_panel_rect(
                width_points,
                height_points,
            )));
        }

        // Jump panel 表示中: `native_jump_panel_rect` の実 rect を使う
        // (= 幅 320pt、top=56pt から hover_bottom まで、画面左端起点)。
        if jump_panel_visible_flag {
            regions.push(rect_to_px(super::overlay_draw::native_jump_panel_rect(
                height_points,
            )));
        }

        // ClickToShow callout は activation zone ではなく実際にクリックする UI なので、
        // 表示中の細い bar rect だけを HUD HWND region に含める。
        for rect in native_panel_callout_hud_rects(
            width_points,
            height_points,
            left_callout_visible,
            right_callout_visible,
            self.vst3_panel_visible(),
        )
        .into_iter()
        .flatten()
        {
            regions.push(rect_to_px(rect));
        }

        // Phase 3 Step 3h: the handle itself is a HUD HWND input region. The
        // OS hit-test therefore routes touch to the HUD-owned widget stream;
        // presenter tap-zone geometry is not used to resolve the handle.
        for rect in self.touch_panel_handle_rects().into_iter().flatten() {
            regions.push(rect_to_px(rect));
        }

        // 実機修正 (2026-05-12 P2): Perf overlay 表示中は perf rect を region に
        // 追加 (= ユーザー報告「Perlグラフが panel に重なる部分しか見えない」対応)。
        // perf overlay は `origin=(14, 14)` で width 300-460pt、height 158pt
        // (`overlay_draw.rs:12-26` 参照)。region 外だと SetWindowRgn で clip される。
        if self.perf_visible {
            let perf_w = width_points.min(460.0).max(300.0);
            regions.push(rect_to_px(egui::Rect::from_min_size(
                egui::pos2(14.0, 14.0),
                egui::vec2(perf_w, 158.0),
            )));
        }

        // Checkmark indicator: passive UI だが、HUD HWND は SetWindowRgn 外の DComp
        // 描画も OS 側で clip する。右パネル等の region が重なった時だけ見える
        // regression を避けるため、描画側と同じ rect を小さく追加する。
        if self.video_checked && !tile_overlay_visible && !navigation_preview_visible {
            let top = if panel_chrome_visible { 68.0 } else { 28.0 };
            regions.push(rect_to_px(super::overlay_draw::native_checkmark_rect(
                width_points,
                top,
            )));
        }

        if self.toast.is_some() {
            if let Some(rect) = self.last_drawn_toast_rect {
                regions.push(rect_to_px(rect));
            } else {
                // 初回フレーム fallback: draw_native_toast が actual rect を記録する前に
                // region 計算だけ走る場合でも、toast が他 UI の region だけに切り抜かれて
                // 表示されないよう中央帯を確保する。次回描画後は実 rect に置き換わる。
                let toast_w = width_points.min(760.0).max(320.0);
                let toast_h = 92.0;
                regions.push(rect_to_px(egui::Rect::from_center_size(
                    egui::pos2(width_points * 0.5, height_points * 0.5),
                    egui::vec2(toast_w, toast_h),
                )));
            }
        }

        if let Some(picker) = self.ring_picker_overlay.as_ref() {
            let rect = self.last_drawn_ring_picker_rect.unwrap_or_else(|| {
                super::overlay_draw::native_ring_picker_overlay_rect(
                    width_points,
                    height_points,
                    picker,
                )
            });
            let rect_px = rect_to_px(rect.expand(4.0));
            if rect_px.left < rect_px.right && rect_px.top < rect_px.bottom {
                regions.push(rect_px);
            }
        }

        if self.ring_picker_overlay.is_none()
            && let Some(guide) = self.ring_guide_overlay.as_ref()
        {
            let rect = self.last_drawn_ring_guide_rect.unwrap_or_else(|| {
                super::overlay_draw::native_ring_guide_overlay_rect(
                    width_points,
                    height_points,
                    self.pixels_per_point,
                    guide,
                )
            });
            let rect_px = rect_to_px(rect.expand(4.0));
            if rect_px.left < rect_px.right && rect_px.top < rect_px.bottom {
                regions.push(rect_px);
            }
        }

        // VST3 panel: ドラッグ可能化 (= 2026-05-12 A) に伴い、`last_drawn_vst3_panel_rect`
        // (実描画後の actual rect) を優先で region に使う。`None` の場合は `native_vst3_panel_rect`
        // (デフォルト位置) に fallback。これで panel がデフォルト位置にあってもドラッグ後でも
        // region が描画位置と一致する。
        if let Some(panel) = self.vst3_panel.as_ref() {
            if panel.visible {
                let rect = self.last_drawn_vst3_panel_rect.unwrap_or_else(|| {
                    super::overlay_draw::native_vst3_panel_rect(width_points, height_points, panel)
                });
                regions.push(rect_to_px(rect));
            }
        }

        if self.video_speed_popup_open {
            let rect = self.last_drawn_speed_popup_rect.unwrap_or_else(|| {
                let popup_w = 356.0_f32.min((width_points - 16.0).max(180.0));
                let popup_h = 74.0;
                let popup_x = (width_points - popup_w - 8.0).max(8.0);
                let popup_y = (height_points - HUD_BOTTOM_HEIGHT - popup_h - 6.0).max(8.0);
                egui::Rect::from_min_size(
                    egui::pos2(popup_x, popup_y),
                    egui::vec2(popup_w, popup_h),
                )
            });
            let rect_px = rect_to_px(rect.expand(4.0));
            if rect_px.left < rect_px.right && rect_px.top < rect_px.bottom {
                regions.push(rect_px);
            }
        }

        // Bookmark title editor: center modal。`draw_native_bookmark_title_editor` の
        // 実描画 rect (`last_drawn_bookmark_editor_rect`) をそのまま region にする。
        //
        // 旧版は画面中心固定の概算 500×100pt だったが、ダイアログは
        // `pos.y = (H - dialog_h) * 0.5` (dialog_h=142 はレイアウト用の過大見積もり) に
        // 配置され、実コンテンツ高さ (~115pt) との差でダイアログ中心が画面中心より
        // 上にずれる。概算 region は画面中心 + 高さ 100pt だったため、ダイアログ上端
        // (=「ブックマーク名」ラベル) が SetWindowRgn でクリップされていた
        // (= 2026-05-14 ユーザー報告「上が欠ける」)。
        if self.bookmark_title_edit.is_some() {
            let rect = self.last_drawn_bookmark_editor_rect.unwrap_or_else(|| {
                // 初回フレーム fallback: 実描画前は draw 側と同じ式で概算する。
                // dialog_h=142 は実コンテンツ高さの過大見積もりなので、この rect は
                // ダイアログ全体を内包する。
                let dialog_w = 360.0_f32.min((width_points - 32.0).max(260.0));
                let dialog_h = 142.0;
                egui::Rect::from_min_size(
                    egui::pos2(
                        (width_points - dialog_w) * 0.5,
                        (height_points - dialog_h) * 0.5,
                    ),
                    egui::vec2(dialog_w, dialog_h),
                )
            });
            let rect_px = rect_to_px(rect.expand(2.0));
            if rect_px.left < rect_px.right && rect_px.top < rect_px.bottom {
                regions.push(rect_px);
            }
        }

        // 一括ブックマーク登録ダイアログ: 大きめの中央モーダル。
        // **実描画した rect を最優先で使う**。ダイアログ高さは「確認削除モード」のあるなし /
        // TextEdit の表示行数 / DPI でかなり変動するため、初回フレーム (実描画前) のみ
        // shared な算出関数で fallback (Codex C6: 旧コードは fallback と実描画で式が
        // 乖離して下部ボタンが SetWindowRgn 外に落ちていた)。
        if self.bulk_bookmark_dialog.is_some() {
            let rect = self
                .last_drawn_bulk_bookmark_dialog_rect
                .unwrap_or_else(|| {
                    let (dialog_w, dialog_h) =
                        super::overlay_draw::native_bulk_bookmark_dialog_size(
                            width_points,
                            height_points,
                        );
                    egui::Rect::from_min_size(
                        egui::pos2(
                            (width_points - dialog_w) * 0.5,
                            (height_points - dialog_h) * 0.5,
                        ),
                        egui::vec2(dialog_w, dialog_h),
                    )
                });
            let rect_px = rect_to_px(rect.expand(2.0));
            if rect_px.left < rect_px.right && rect_px.top < rect_px.bottom {
                regions.push(rect_px);
            }
        }

        if self.shortcut_help_open {
            let rect = self.last_drawn_shortcut_help_rect.unwrap_or_else(|| {
                let (dialog_w, dialog_h) = super::overlay_draw::native_shortcut_help_dialog_size(
                    width_points,
                    height_points,
                );
                egui::Rect::from_min_size(
                    egui::pos2(
                        (width_points - dialog_w) * 0.5,
                        (height_points - dialog_h) * 0.5,
                    ),
                    egui::vec2(dialog_w, dialog_h),
                )
            });
            let rect_px = rect_to_px(rect.expand(2.0));
            if rect_px.left < rect_px.right && rect_px.top < rect_px.bottom {
                regions.push(rect_px);
            }
        }

        // Normalize progress / scan blocker: 全画面被覆 (= scan 中はモーダル cancel ボタン操作のため)。
        if matches!(
            self.normalize_state.ui_state,
            crate::video::normalize_types::NormalizeUiState::Scanning
        ) {
            regions.push(RECT {
                left: 0,
                top: 0,
                right: width_px,
                bottom: height_px,
            });
        }

        // Navigation preview / tile overlay: HUD HWND 全面で静止画または tile grid を描く。
        if navigation_preview_visible || self.tile_overlay.is_some() {
            regions.push(RECT {
                left: 0,
                top: 0,
                right: width_px,
                bottom: height_px,
            });
        }

        // Seek hover thumbnail: **直近 egui run で実描画した rect を直接使う** (Codex 助言
        // 「描画側の preview_rect をそのまま保存して使う」反映、2026-05-12 P1 #2)。
        //
        // ## なぜ ptr.x ベース再計算は NG か
        //
        // 旧版は `compute_hud_regions` 内で `(ptr.x - preview_image_w * 0.5).clamp(...)` で
        // 再計算していたが、実描画は `bar_rect.min.x + bar_rect.width() * frac` (=
        // hover_preview_target_secs ベース) で計算する。両者が乖離するケース:
        //   - cursor が seek bar を離れ thumbnail 上に移動 → `seek_resp.hovered()` 不成立
        //   - `hover_preview_target_secs` は cursor 離脱前の値で固定 (overlay_draw:4170-4171)
        //   - cursor をサムネ上で左右に動かす:
        //     - 描画 rect: target_secs ベースなので **固定**
        //     - region rect: ptr.x ベースなので **cursor 追従**
        //   - 結果: サムネ画像 (= 描画 rect) は固定だが region が動く → region 外に出た部分が
        //     SetWindowRgn で clip されて「枠だけ動いて見える」症状 (= ユーザー報告)
        //
        // `last_drawn_preview_rect` には draw が「実際にこのフレームで描いた rect」が入って
        // いる (= None なら描画なし)。これをそのまま region に変換する。
        if let Some(rect) = self.last_drawn_preview_rect {
            regions.push(rect_to_px(rect));
        }

        // egui tooltip は Order::Tooltip の floating Area として下 HUD / panel の外側に
        // 描かれる。HUD HWND の SetWindowRgn に実 rect を含めないと、DComp には描けても
        // HWND region 外で物理的に clip される。
        let mut tooltip_layers: Vec<egui::LayerId> = self.egui_ctx.memory(|mem| {
            mem.areas()
                .visible_layer_ids()
                .into_iter()
                .filter(|layer_id| layer_id.order == egui::Order::Tooltip)
                .collect()
        });
        tooltip_layers.sort_by_key(|layer_id| layer_id.id.value());
        for layer_id in tooltip_layers {
            if let Some(state) = egui::AreaState::load(&self.egui_ctx, layer_id.id) {
                let rect = state.rect();
                if rect.is_finite() && rect.is_positive() {
                    let rect_px = rect_to_px(rect);
                    if rect_px.left < rect_px.right && rect_px.top < rect_px.bottom {
                        regions.push(rect_px);
                    }
                }
            }
        }

        regions
    }

    fn input_routing(&self) -> NativeOverlayInputRouting {
        NativeOverlayInputRouting {
            wants_pointer_input: self.wants_pointer_input,
            wants_keyboard_input: self.wants_keyboard_input,
            text_input_active: self.text_input_active(),
            hud_dimmed: self.hud_dimmed,
            // consumed_wheel は commands を見て render_if_dirty 側で設定する。
            ..Default::default()
        }
    }

    fn text_input_active(&self) -> bool {
        self.bookmark_title_edit.is_some()
            || self.bulk_bookmark_dialog.is_some()
            || self.tag_picker_open
    }

    fn can_open_shortcut_help(&self) -> bool {
        !self.text_input_active()
            && !self.ime_input_active()
            && !self.shortcut_help_open
            && !matches!(
                self.normalize_state.ui_state,
                crate::video::normalize_types::NormalizeUiState::Scanning
            )
    }

    fn ime_input_active(&self) -> bool {
        crate::ime_focus::ime_input_active_with_pending_events(
            &self.egui_ctx,
            egui::ViewportId::ROOT,
            &self.pending_events,
        )
    }

    fn ime_composing(&self) -> bool {
        crate::ime_focus::ime_composing_with_pending_events(
            &self.egui_ctx,
            egui::ViewportId::ROOT,
            &self.pending_events,
        )
    }

    fn maybe_claim_text_input_focus(&mut self) -> Option<NativeWindowIntent> {
        if !self.text_input_active() {
            self.last_text_input_focus_claim_at = None;
            return None;
        }

        let focus_state = self.window_observation.focus;
        let target_hwnd = focus_state.target_id;
        let thread_focus_hwnd = focus_state.thread_focus_id;
        let foreground_is_current_process = focus_state.foreground_is_current_process;
        if !should_claim_text_input_focus(
            true,
            target_hwnd,
            thread_focus_hwnd,
            foreground_is_current_process,
        ) {
            // 他アプリが foreground の間は何もしない。戻ってきた直後の tick で即座に
            // claim できるよう、抑制タイマはここで解除しておく。
            if thread_focus_hwnd == target_hwnd || !foreground_is_current_process {
                self.last_text_input_focus_claim_at = None;
            }
            return None;
        }

        let now = Instant::now();
        if self
            .last_text_input_focus_claim_at
            .is_some_and(|prev| now.duration_since(prev) < TEXT_INPUT_FOCUS_CLAIM_MIN_INTERVAL)
        {
            return None;
        }
        self.last_text_input_focus_claim_at = Some(now);
        Some(NativeWindowIntent::ClaimTextInputFocus)
    }

    fn hud_visible(&self) -> bool {
        let overlay_height_points = self.height as f32 / self.pixels_per_point;
        Self::native_hud_bottom_visible_from_hover(
            self.visibility_hover_pos(),
            overlay_height_points,
            self.external_drag_in_progress,
            self.native_touch.chrome_latched(),
            self.bottom_lock.bar_locked(),
        )
    }

    fn top_bar_visible(&self) -> bool {
        Self::native_hud_top_visible_from_hover(
            self.visibility_hover_pos(),
            self.top_bar_visible,
            self.external_drag_in_progress,
            self.native_touch.chrome_latched(),
            self.top_bar_locked,
        )
    }

    fn top_bar_hover_visible(&self) -> bool {
        Self::native_hud_top_visible_from_hover(
            self.visibility_hover_pos(),
            self.top_bar_visible,
            self.external_drag_in_progress,
            self.native_touch.chrome_latched(),
            false,
        )
    }

    fn vst3_panel_visible(&self) -> bool {
        self.vst3_panel.as_ref().is_some_and(|panel| panel.visible)
    }

    /// 左右パネルの端ホバー開閉ラッチをフレーム先頭で更新する (実機 FB 2026-07)。
    ///  - 開くトリガ = 画面端 5% (`panel_edge_trigger_px`) の細いストリップ。パネル幅ぶん
    ///    (右 430 / 左 320pt) の広い当たり判定は中央のクリックを食うため。
    ///  - 維持 = 描画パネル矩形 + ヒステリシス余白 (`panel_hover_sustain_px`)。パネル内端を
    ///    わずかに越えても即閉じない (項目クリックへ移動する動線を確保)。
    /// egui / 音楽ビュー側の二段ラッチと同じモデル。`render_once` の先頭で 1 回だけ呼ぶ。
    fn update_side_panel_hover_latches(&mut self) {
        if self.side_panel_mode.normalized() != FsSidePanelMode::Hover {
            self.right_panel_hover_latched = false;
            self.jump_panel_hover_latched = false;
            return;
        }
        let overlay_width_points = self.width as f32 / self.pixels_per_point;
        let overlay_height_points = self.height as f32 / self.pixels_per_point;
        let seek_strip_visible = self.seek_strip.is_some()
            && self.tile_overlay.is_none()
            && self.navigation_preview.is_none();
        let hover_bottom = native_panel_hover_bottom(overlay_height_points, seek_strip_visible);
        let trigger = crate::ui_helpers::panel_edge_trigger_px(overlay_width_points);
        let margin = crate::ui_helpers::panel_hover_sustain_px(overlay_width_points);
        let in_band = |p: egui::Pos2| p.y >= 0.0 && p.y <= hover_bottom;

        let right_panel = native_metadata_panel_rect(overlay_width_points, overlay_height_points);
        let right_open = self
            .pointer_pos
            .is_some_and(|p| p.x >= overlay_width_points - trigger && in_band(p));
        let right_sustain = self.right_panel_hover_latched
            && self
                .pointer_pos
                .is_some_and(|p| right_panel.expand(margin).contains(p));
        self.right_panel_hover_latched =
            self.pointer_pos.is_some() && (right_open || right_sustain);

        let left_panel = native_jump_panel_rect(overlay_height_points);
        let left_open = self
            .pointer_pos
            .is_some_and(|p| p.x <= trigger && in_band(p));
        let left_sustain = self.jump_panel_hover_latched
            && self
                .pointer_pos
                .is_some_and(|p| left_panel.expand(margin).contains(p));
        self.jump_panel_hover_latched = self.pointer_pos.is_some() && (left_open || left_sustain);
    }

    fn right_panel_visible(&self) -> bool {
        let Some(metadata) = self.video_metadata.as_ref() else {
            return false;
        };
        let metadata_available = metadata.probe_info_available
            || !metadata.shortcut_tags.is_empty()
            || !metadata.current_tags.is_empty()
            || !metadata.tag_choices.is_empty();
        // 端ホバー判定は二段ラッチ (update_side_panel_hover_latches がフレーム先頭で更新)。
        native_right_panel_visible_from_inputs(NativeRightPanelVisibilityInputs {
            shortcut_help_open: self.shortcut_help_open,
            external_drag_in_progress: self.external_drag_in_progress,
            vst3_panel_visible: self.vst3_panel_visible(),
            metadata_available,
            video_speed_popup_open: self.video_speed_popup_open,
            hover_preview_active: self.hover_preview_target_secs.is_some(),
            tag_picker_open: self.tag_picker_open,
            panorama_active: self.panorama_pose.is_some(),
            pointer_in_hover_rect: self.right_panel_hover_latched,
            side_panel_mode: self.side_panel_mode,
            info_panel_open: self.info_panel_open,
        })
    }

    fn jump_panel_visible(&self) -> bool {
        // 端ホバー判定は二段ラッチ (update_side_panel_hover_latches がフレーム先頭で更新)。
        native_jump_panel_visible_from_inputs(NativeJumpPanelVisibilityInputs {
            shortcut_help_open: self.shortcut_help_open,
            vst3_panel_visible: self.vst3_panel_visible(),
            video_speed_popup_open: self.video_speed_popup_open,
            hover_preview_active: self.hover_preview_target_secs.is_some(),
            pointer_in_hover_rect: self.jump_panel_hover_latched,
            panorama_active: self.panorama_pose.is_some(),
            side_panel_mode: self.side_panel_mode,
            left_panel_open: self.left_panel_open,
        })
    }

    fn metadata_available(&self) -> bool {
        self.video_metadata.as_ref().is_some_and(|metadata| {
            metadata.probe_info_available
                || !metadata.shortcut_tags.is_empty()
                || !metadata.current_tags.is_empty()
                || !metadata.tag_choices.is_empty()
        })
    }

    fn touch_panel_handle_rects(&self) -> [Option<egui::Rect>; 2] {
        let width = self.width as f32 / self.pixels_per_point;
        let height = self.height as f32 / self.pixels_per_point;
        native_touch_panel_handle_hud_rects(
            width,
            height,
            NativeTouchPanelHandleInputs {
                chrome_latched: self.native_touch.chrome_latched(),
                blocked: self.audio_only
                    || self.external_drag_in_progress
                    || self.shortcut_help_open
                    || self.vst3_panel_visible()
                    || self.video_speed_popup_open
                    || self.hover_preview_target_secs.is_some()
                    || self.tile_overlay.is_some()
                    || self.seek_strip.is_some()
                    || self.navigation_preview.is_some(),
                left_panel_open: self.left_panel_open.is_open(),
                right_panel_open: self.info_panel_open.is_open(),
                right_panel_available: self.metadata_available(),
            },
        )
    }

    /// ClickToShow の最端 hover で表示する左右 callout。panel 本体の modal gate と揃える。
    fn side_panel_callout_visibility(&self) -> (bool, bool) {
        if self.panorama_pose.is_some()
            || self.side_panel_mode.normalized() != FsSidePanelMode::ClickToShow
            || self.external_drag_in_progress
            || self.shortcut_help_open
            || self.vst3_panel_visible()
            || self.video_speed_popup_open
            || self.hover_preview_target_secs.is_some()
            || self.tile_overlay.is_some()
            || self.seek_strip.is_some()
            || self.navigation_preview.is_some()
        {
            return (false, false);
        }
        let Some(pointer) = self.visibility_hover_pos() else {
            return (false, false);
        };
        let width = self.width as f32 / self.pixels_per_point;
        let height = self.height as f32 / self.pixels_per_point;
        let left = crate::ui_helpers::callout_hit(
            native_panel_callout_edge_rect(width, height, true),
            Some(pointer),
        );
        let right = self.metadata_available()
            && crate::ui_helpers::callout_hit(
                native_panel_callout_edge_rect(width, height, false),
                Some(pointer),
            );
        (left, right)
    }

    fn pointer_over_scroll_panel(&self, pos: egui::Pos2) -> bool {
        let overlay_width_points = self.width as f32 / self.pixels_per_point;
        let overlay_height_points = self.height as f32 / self.pixels_per_point;
        (self.jump_panel_visible() && native_jump_panel_rect(overlay_height_points).contains(pos))
            || (self.right_panel_visible()
                && native_metadata_panel_rect(overlay_width_points, overlay_height_points)
                    .contains(pos))
    }

    fn configure(&self) -> Result<(), String> {
        // `Surface::configure` may internally wait for queue work. Serialize it
        // against this epoch's submission span across every overlay window.
        self.gpu_epoch.ensure_alive("surface configure")?;
        let _configure_guard = self.gpu_epoch.configure_guard();
        self.gpu_epoch.ensure_alive("surface configure")?;
        self.surface.configure(
            self.gpu_epoch.device(),
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: self.format,
                width: self.width,
                height: self.height,
                present_mode: self.present_mode,
                desired_maximum_frame_latency: 1,
                alpha_mode: self.alpha_mode,
                view_formats: vec![],
            },
        );
        Ok(())
    }

    fn set_visual_attached(&mut self, attached: bool) -> Result<(), String> {
        if self.visual_attached == attached {
            return Ok(());
        }
        let _operation = self
            .health
            .begin_render_operation(NativeRenderOperation::DCompCommit, self.window_epoch);
        unsafe {
            if attached {
                // CP3 P1 #3 反映: `after_visual` が `Some(v)` なら presenter フォールバック経路
                // (= video visual の後ろに挟む)、`None` なら HUD HWND の DComp root に
                // 単独配置する。後者は HUD root に他の visual がないので `None` で OK。
                let result = match &self.after_visual {
                    Some(v) => self.root_visual.AddVisual(&self.visual, true, v),
                    None => {
                        self.root_visual
                            .AddVisual(&self.visual, true, None::<&IDCompositionVisual>)
                    }
                };
                result
                    .map_err(|e| format!("IDCompositionVisual::AddVisual egui overlay: {e:?}"))?;
            } else {
                self.root_visual.RemoveVisual(&self.visual).map_err(|e| {
                    format!("IDCompositionVisual::RemoveVisual egui overlay: {e:?}")
                })?;
            }
            self.dcomp_device
                .Commit()
                .map_err(|e| format!("IDCompositionDevice::Commit egui overlay visual: {e:?}"))?;
        }
        self.visual_attached = attached;
        log_event(
            "egui_overlay_visual",
            &[("attached", Value::from(self.visual_attached))],
        );
        Ok(())
    }

    fn render_once(
        &mut self,
    ) -> Result<(Vec<NativeOverlayCommand>, Vec<NativeWindowIntent>), String> {
        let mut window_intents = Vec::new();
        let render_t0 = Instant::now();
        self.gpu_epoch.ensure_alive("overlay draw")?;
        let first_render = self.render_count == 0;
        if self
            .toast
            .as_ref()
            .is_some_and(|toast| toast.started_at.elapsed() > toast.linger)
        {
            self.toast = None;
        }
        let seek_status_active = self.update_seek_status_for_render(render_t0);
        self.sync_hover_thumbnail_texture();
        self.sync_navigation_preview_texture();
        self.sync_tile_overlay_textures();
        self.sync_seek_strip_textures();
        self.sync_jump_entry_textures();
        let ppp = self.pixels_per_point;
        let event_count = self.event_count;
        let pointer_pos = self.pointer_pos;
        let visibility_hover_pos = self.visibility_hover_pos();
        // 左右パネルの端ホバー開閉ラッチをフレーム先頭で更新する (実機 FB 2026-07)。以降の
        // jump_panel_visible() / right_panel_visible() はこのラッチを読む。
        self.update_side_panel_hover_latches();
        // hover_preview_target_secs はシークバー Area (= bottom_hud_visible) 内でしか
        // 更新されない。pointer が hud 領域から外れた状態で Some が居座ると
        // jump_panel_visible() / right_panel_visible() を false に固定し続ける
        // ことがある。
        // hud 領域外なら preview UI 自体描画されないので、ここで先に倒す。
        if self.hover_preview_target_secs.is_some() && !self.hud_visible() {
            self.clear_seek_preview();
        }
        let overlay_width_points = self.width as f32 / ppp;
        let overlay_height_points = self.height as f32 / ppp;
        let (left_callout_visible, right_callout_visible) = self.side_panel_callout_visibility();
        let touch_panel_handle_rects = self.touch_panel_handle_rects();
        let touch_panel_handles_visible = touch_panel_handle_rects.iter().any(Option::is_some);
        let position_secs = self.video_position_secs;
        let duration_secs = self.video_duration_secs;
        let position_controls_available =
            crate::video::seek_strip::video_position_controls_available(duration_secs);
        let seek_strip_unavailable_tooltip = if !position_controls_available {
            Some("長さの情報がない動画では使えません")
        } else {
            match self.seek_strip_material_availability {
                crate::video::seek_strip::SeekStripMaterialAvailability::Unavailable(reason) => {
                    Some(reason.tooltip())
                }
                _ => None,
            }
        };
        let seek_strip_material_available = seek_strip_unavailable_tooltip.is_none();
        // P10-2: now-playing バナー (現在再生中のチャプター/ブックマーク) は左パネル
        // 表示中だけ出す。`jump_panel_visible()` は pointer hover で決まるため、
        // closure 内で再計算するのではなく、ここで bool として取って move する。
        let jump_panel_visible_for_banner = self.jump_panel_visible();
        let is_playing = self.video_is_playing;
        let volume = self.video_volume;
        let muted = self.video_muted;
        let limiter_ceiling_hit = self.limiter_indicator_visible_at(render_t0);
        let playback_speed = self.video_playback_speed;
        let checked = self.video_checked;
        let loop_enabled = self.video_loop_enabled;
        let loop_mode = self.video_loop_mode;
        let continuous_mode = self.video_continuous_mode;
        let first_frame_presented = self.first_frame_presented;
        let video_error = self.video_error.clone();
        let preparing_status = self.preparing_status;
        let toast = self.toast.clone();
        let hover_thumbnail = self.hover_thumbnail.clone();
        let hover_texture_id = self.hover_texture.as_ref().map(|texture| texture.id());
        let hover_preview_pinned = self.hover_preview_pinned;
        let timeline_markers = self.timeline_markers.clone();
        let jump_entries = self.jump_entries.clone();
        let creative_lut_choices = self.creative_lut_choices.clone();
        let mut video_adjustments = self.video_adjustments.clone();
        let mut video_scale_filter = self.video_scale_filter;
        let mut video_downscale_smoothing_percent = self.video_downscale_smoothing_percent;
        let video_scale_outside_note = self.video_scale_outside_note.clone();
        let mut video_anime4k_budget = self.video_anime4k_budget;
        let video_anime4k_status = self.video_anime4k_status.clone();
        let video_preset_slots = self.video_preset_slots.clone();
        let mut video_left_panel_tab = self.video_left_panel_tab;
        let mut video_adjustment_tab = self.video_adjustment_tab;
        let mut bookmark_title_edit = self.bookmark_title_edit.take();
        let mut bulk_bookmark_dialog = self.bulk_bookmark_dialog.take();
        let video_metadata = self.video_metadata.clone();
        let panorama_pose = self.panorama_pose;
        let shortcut_labels = video_metadata.as_ref().map(|metadata| &metadata.shortcuts);
        let mut shortcut_help_open = self.shortcut_help_open;
        let fallback_file_name = self.fallback_file_name.clone();
        let navigation_preview = self.navigation_preview.clone();
        let navigation_preview_texture_id = self
            .navigation_preview_texture
            .as_ref()
            .map(|(_, texture)| texture.id());
        // 音量ノーマライズ overlay state (Copy 型なので clone 不要)
        let normalize_state_snap = self.normalize_state;
        let vst3_panel = self.vst3_panel.clone();
        let mut last_emitted_vst3_panel_pos = self.last_emitted_vst3_panel_pos;
        let tile_overlay = self.tile_overlay.clone();
        let seek_strip = self.seek_strip.clone();
        let ring_picker_overlay = self.ring_picker_overlay.clone();
        let ring_guide_overlay = self.ring_guide_overlay.clone();
        let tile_texture_ids: HashMap<usize, egui::TextureId> = self
            .tile_textures
            .iter()
            .map(|(idx, (_, texture))| (*idx, texture.id()))
            .collect();
        let seek_strip_texture_ids: HashMap<usize, egui::TextureId> = self
            .seek_strip_textures
            .iter()
            .map(|(index, (_, texture))| (*index, texture.id()))
            .collect();
        let seek_strip_wave_texture_id = self
            .seek_strip_wave_texture
            .as_ref()
            .map(|(_, texture)| texture.id());
        let jump_texture_ids: HashMap<usize, egui::TextureId> = self
            .jump_textures
            .iter()
            .map(|(idx, (_, texture))| (*idx, texture.id()))
            .collect();
        let perf_visible = self.perf_visible;
        let vst3_available = self.vst3_available;
        let audio_only = self.audio_only;
        let ui_scale = self.ui_scale;
        let touch_first_run_help_visible = self.native_touch.first_run_help_visible();
        let side_panel_mode = self.side_panel_mode;
        let top_bar_locked = self.top_bar_locked;
        let bottom_lock = self.bottom_lock;
        let vst3_panel_visible = vst3_panel.as_ref().is_some_and(|panel| panel.visible);
        let hud_dimmed = self.hud_dimmed;
        let perf_latest = self.perf_latest;
        let perf_history: Vec<_> = self.perf_history.iter().copied().collect();
        let hud_visible = self.hud_visible();
        let jump_panel_visible = self.jump_panel_visible();
        let top_bar_hover_visible = self.top_bar_hover_visible();
        let top_bar_visible = self.top_bar_visible();
        let right_panel_visible = self.right_panel_visible();
        let tile_overlay_visible = tile_overlay.is_some();
        let seek_strip_visible = seek_strip.is_some() && !tile_overlay_visible;
        let navigation_preview_visible = navigation_preview.is_some();
        let raw_seek_status_visible = seek_status_active
            && !tile_overlay_visible
            && !navigation_preview_visible
            && first_frame_presented
            && video_error.is_none();
        let toast_visible = toast.is_some();
        let bookmark_title_edit_visible = bookmark_title_edit.is_some();
        let bulk_bookmark_dialog_visible = bulk_bookmark_dialog.is_some();
        let ring_picker_visible = ring_picker_overlay.is_some();
        let ring_guide_visible = ring_guide_overlay.is_some() && !ring_picker_visible;
        let side_panel_visible = !tile_overlay_visible
            && !navigation_preview_visible
            && (jump_panel_visible || right_panel_visible);
        let bar_visibility = Self::native_bar_visibility_snapshot(
            hud_visible,
            top_bar_visible,
            top_bar_hover_visible,
            side_panel_visible,
            tile_overlay_visible,
            navigation_preview_visible,
        );
        let panel_chrome_visible = bar_visibility.top_bar_visible;
        // normalize scanning 中は HUD/Toast が無くても progress UI を描く必要がある。
        let normalize_scanning = matches!(
            normalize_state_snap.ui_state,
            crate::video::normalize_types::NormalizeUiState::Scanning
        );
        let seek_status_visible = raw_seek_status_visible;
        let status_visible = video_error.is_some() || !first_frame_presented || seek_status_visible;
        let bottom_hud_visible = bar_visibility.bottom_hud_visible;
        let perf_origin = egui::pos2(14.0, 14.0);
        // Codex 2周目 P1: normalize_scanning も overlay_visible / cursor_blocking_overlay_visible
        // に含める。さもないと HUD/Toast 等が出ていない状態で `if !overlay_visible { return; }`
        // 早期 return に入って progress UI まで到達しない。
        let overlay_visible = navigation_preview_visible
            || tile_overlay_visible
            || seek_strip_visible
            || bottom_hud_visible
            || panel_chrome_visible
            || perf_visible
            || (!tile_overlay_visible && !navigation_preview_visible && checked)
            || status_visible
            || toast_visible
            || bookmark_title_edit_visible
            || bulk_bookmark_dialog_visible
            || shortcut_help_open
            || vst3_panel_visible
            || ring_picker_visible
            || ring_guide_visible
            || left_callout_visible
            || right_callout_visible
            || touch_panel_handles_visible
            || touch_first_run_help_visible
            || normalize_scanning;
        // カーソル auto-hide の判定用: チェックマークのような「受動表示」(ユーザーが
        // 操作する対象ではなく単なる状態インジケータ) は countdown をブロックしない。
        // 静止画側 `fs_ui_is_clean` がチェック状態を考慮しないのと挙動を揃える。
        // tile overlay は受動表示の側面が強いが、サムネがクリックで操作可能なので
        // カーソル可視を維持する側に含める。
        // navigation preview は動画→動画 source swap 中の受動表示なので blocking に
        // 含めない。含めるとキー操作の上下移動だけで非表示カーソルが復活する。
        // マウス/ホイール操作の動画移動は App 側で明示的に cursor activity を入れる。
        //
        // 実機修正 (2026-05-12, Codex 助言 #3): **HUD activation zone (上端 76pt /
        // 下端 220pt)** に cursor が入っていれば auto-hide を抑制する。バーが「ふっと
        // 出る瞬間にカーソルが一瞬消える」症状は、cursor が activation zone に入った
        // フレームと top_bar_visible が true になるフレームが 1 フレームずれて、その間に
        // `cursor_should_hide = true` が成立して `SetCursor(None)` が呼ばれることが原因。
        // activation zone 内では auto-hide を打ち切ることで予防する。
        let in_top_activation_zone = pointer_pos.map(|p| p.y <= 76.0).unwrap_or(false);
        let in_bottom_activation_zone = pointer_pos
            .map(|p| p.y >= overlay_height_points - 220.0)
            .unwrap_or(false);
        let cursor_blocking_status_visible = status_visible && !navigation_preview_visible;
        let cursor_blocking_overlay_visible = tile_overlay_visible
            || seek_strip_visible
            || bottom_hud_visible
            || panel_chrome_visible
            || cursor_blocking_status_visible
            || toast_visible
            || bookmark_title_edit_visible
            || bulk_bookmark_dialog_visible
            || shortcut_help_open
            || vst3_panel_visible
            || ring_picker_visible
            || ring_guide_visible
            || touch_first_run_help_visible
            || normalize_scanning
            || in_top_activation_zone
            || in_bottom_activation_zone
            || left_callout_visible
            || right_callout_visible
            || touch_panel_handles_visible;
        let pending_event_count = self.pending_events.len();
        let mut commands = std::mem::take(&mut self.pending_overlay_commands);
        let mut last_seek_target_secs = self.last_seek_target_secs;
        let mut last_thumbnail_request_secs = self.last_thumbnail_request_secs;
        let mut last_thumbnail_request_at = self.last_thumbnail_request_at;
        let mut hover_preview_target_secs = self.hover_preview_target_secs;
        let mut hover_preview_anchor_x = self.hover_preview_anchor_x;
        let mut video_speed_popup_open = self.video_speed_popup_open;
        let mut frame_step_hold = self.frame_step_hold;
        let mut seek_row_gesture = self.seek_row_gesture;
        let mut seek_strip_drag_origin = self.seek_strip_drag_origin;
        let mut last_seek_strip_window_request = self.last_seek_strip_window_request;
        // 実機修正 (2026-05-12 P1 #2): 実描画した preview_rect を記録して
        // `compute_hud_regions` に渡す (= region を draw rect と完全同期させる)。
        let mut last_drawn_preview_rect: Option<egui::Rect> = None;
        // 実機修正 (2026-05-12 A): VST3 設定パネルをドラッグ可能化 (`.movable(true)`)。
        // ドラッグ後の actual rect を記録して region に追従させる。
        let mut last_drawn_vst3_panel_rect: Option<egui::Rect> = None;
        let mut last_drawn_toast_rect: Option<egui::Rect> = None;
        let mut last_drawn_speed_popup_rect: Option<egui::Rect> = None;
        let mut last_drawn_bookmark_editor_rect: Option<egui::Rect> = None;
        let mut last_drawn_bulk_bookmark_dialog_rect: Option<egui::Rect> = None;
        let mut last_drawn_shortcut_help_rect: Option<egui::Rect> = None;
        let mut last_drawn_ring_picker_rect: Option<egui::Rect> = None;
        let mut last_drawn_ring_guide_rect: Option<egui::Rect> = None;
        let left_panel_open_before = self.left_panel_open;
        let mut left_panel_open = left_panel_open_before;
        if !overlay_visible {
            self.set_visual_attached(false)?;
            last_seek_target_secs = None;
            last_thumbnail_request_secs = None;
            last_thumbnail_request_at = None;
            set_seek_preview_target(
                &mut hover_preview_target_secs,
                &mut hover_preview_anchor_x,
                None,
            );
            frame_step_hold = None;
        }
        let mut raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(overlay_width_points, overlay_height_points),
            )),
            time: Some(self.started_at.elapsed().as_secs_f64()),
            predicted_dt: 1.0 / 60.0,
            modifiers: self.modifiers,
            events: std::mem::take(&mut self.pending_events),
            ..Default::default()
        };
        if let Some(viewport) = raw_input.viewports.get_mut(&egui::ViewportId::ROOT) {
            viewport.native_pixels_per_point = Some(ppp);
        }
        // IME 変換中 (`ime_input_active`) は Enter/Escape を確定/キャンセルキーとして扱わない
        // (変換の確定・取り消しをタグピッカーが奪わないため、静止画側 `dialog_*_pressed` と同方針)。
        let tag_picker_ime_active = self.ime_input_active();
        let tag_picker_enter_allowed = !tag_picker_ime_active;
        let mut seek_strip_area_ran = false;
        let mut seek_strip_preview_hover = NativeSeekStripPreviewHover::Outside;
        let egui_run_t0 = Instant::now();
        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            if !overlay_visible {
                return;
            }
            // タイルモード中は perf overlay を描かない。タイルグリッドは全画面を
            // 不透明黒で塗るので、grid が `Order::Background`・perf が `Order::Middle`
            // だと perf がグリッドの上に乗ってサムネイルとクリック (seek) を塞いでしまう。
            // 旧実装 (grid = Foreground) でも grid の不透明塗りで perf は隠れていたので、
            // タイルモードで perf を出さないのは元の見た目と一致する。
            if perf_visible && tile_overlay.is_none() && navigation_preview.is_none() {
                draw_native_perf_overlay(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    &perf_history,
                    perf_latest,
                    perf_origin,
                );
            }
            if let Some(tile_overlay) = tile_overlay.as_ref() {
                draw_native_tile_overlay(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    tile_overlay,
                    &tile_texture_ids,
                    &mut commands,
                );
                draw_native_top_bar_tile(
                    ctx,
                    overlay_width_points,
                    video_metadata.as_ref(),
                    tile_overlay,
                    hud_dimmed,
                    &mut commands,
                );
                // タイル表示中も境界トースト (= 「末尾です」「次のフォルダが見つかりません」
                // 等) は出す。元実装は早期 return で line 4342 の draw_native_toast に
                // 到達しなかったため、タイル末尾に達してもユーザーへの feedback がゼロだった。
                if let Some(toast) = toast.as_ref() {
                    last_drawn_toast_rect =
                        draw_native_toast(ctx, overlay_width_points, overlay_height_points, toast);
                }
                if let Some(picker) = ring_picker_overlay.as_ref() {
                    last_drawn_ring_picker_rect = draw_native_ring_picker_overlay(
                        ctx,
                        overlay_width_points,
                        overlay_height_points,
                        picker,
                    );
                } else if let Some(guide) = ring_guide_overlay.as_ref() {
                    last_drawn_ring_guide_rect = draw_native_ring_guide_overlay(
                        ctx,
                        overlay_width_points,
                        overlay_height_points,
                        ppp,
                        guide,
                    );
                }
                if shortcut_help_open {
                    if let Some(metadata) = video_metadata.as_ref() {
                        last_drawn_shortcut_help_rect = draw_native_shortcut_help_dialog(
                            ctx,
                            overlay_width_points,
                            overlay_height_points,
                            metadata.shortcut_help.as_ref(),
                            !audio_only,
                            &mut shortcut_help_open,
                        );
                    } else {
                        shortcut_help_open = false;
                    }
                }
                if touch_first_run_help_visible {
                    draw_native_video_touch_first_run_help(
                        ctx,
                        overlay_width_points,
                        overlay_height_points,
                        ui_scale,
                    );
                }
                return;
            }
            if let Some(preview) = navigation_preview.as_ref() {
                draw_native_navigation_preview(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    preview,
                    navigation_preview_texture_id,
                    &mut commands,
                );
                if let Some(toast) = toast.as_ref() {
                    last_drawn_toast_rect =
                        draw_native_toast(ctx, overlay_width_points, overlay_height_points, toast);
                }
                if let Some(picker) = ring_picker_overlay.as_ref() {
                    last_drawn_ring_picker_rect = draw_native_ring_picker_overlay(
                        ctx,
                        overlay_width_points,
                        overlay_height_points,
                        picker,
                    );
                } else if let Some(guide) = ring_guide_overlay.as_ref() {
                    last_drawn_ring_guide_rect = draw_native_ring_guide_overlay(
                        ctx,
                        overlay_width_points,
                        overlay_height_points,
                        ppp,
                        guide,
                    );
                }
                if shortcut_help_open {
                    if let Some(metadata) = video_metadata.as_ref() {
                        last_drawn_shortcut_help_rect = draw_native_shortcut_help_dialog(
                            ctx,
                            overlay_width_points,
                            overlay_height_points,
                            metadata.shortcut_help.as_ref(),
                            !audio_only,
                            &mut shortcut_help_open,
                        );
                    } else {
                        shortcut_help_open = false;
                    }
                }
                if touch_first_run_help_visible {
                    draw_native_video_touch_first_run_help(
                        ctx,
                        overlay_width_points,
                        overlay_height_points,
                        ui_scale,
                    );
                }
                return;
            }
            if let Some(error) = video_error.as_deref() {
                draw_native_center_status(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    "動画を再生できません",
                    Some(error),
                    true,
                );
            } else if !first_frame_presented {
                // フェーズ + 累積バイト数で文言を切り替え (Codex P2 への補足対応):
                // 「メタデータ読込中... NN MB / YY MB」「ストリーム解析中...」など。
                // moov atom 末尾配置の遅い動画でフリーズと誤認されないようにする。
                let title = crate::video::avio_progress::build_preparing_message(preparing_status);
                draw_native_center_status(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    &title,
                    None,
                    false,
                );
            } else if seek_status_visible {
                draw_native_center_status(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    "シーク中...",
                    None,
                    false,
                );
            }
            if panel_chrome_visible {
                draw_native_top_bar(
                    ctx,
                    overlay_width_points,
                    position_secs,
                    duration_secs,
                    video_metadata.as_ref(),
                    panorama_pose,
                    &fallback_file_name,
                    perf_visible,
                    vst3_available,
                    vst3_panel_visible,
                    audio_only,
                    side_panel_mode,
                    top_bar_locked,
                    hud_dimmed,
                    &mut commands,
                );
            }
            if checked {
                draw_native_checkmark(
                    ctx,
                    overlay_width_points,
                    if panel_chrome_visible { 68.0 } else { 28.0 },
                );
            }
            if vst3_panel_visible && let Some(panel) = vst3_panel.as_ref() {
                last_drawn_vst3_panel_rect = draw_native_vst3_panel(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    panel,
                    &mut commands,
                    &mut last_emitted_vst3_panel_pos,
                );
            }
            if let Some(toast) = toast.as_ref() {
                last_drawn_toast_rect =
                    draw_native_toast(ctx, overlay_width_points, overlay_height_points, toast);
            }
            if let Some(picker) = ring_picker_overlay.as_ref() {
                last_drawn_ring_picker_rect = draw_native_ring_picker_overlay(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    picker,
                );
            } else if let Some(guide) = ring_guide_overlay.as_ref() {
                last_drawn_ring_guide_rect = draw_native_ring_guide_overlay(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    ppp,
                    guide,
                );
            }
            let tag_picker_enter_pressed =
                tag_picker_enter_allowed && ctx.input(|i| i.key_pressed(egui::Key::Enter));
            // Escape はピッカーを閉じる (静止画右パネルのタグピッカーと挙動を揃える)。
            // IME 変換キャンセルの Escape は `tag_picker_enter_allowed` で除外済み。
            let tag_picker_escape_pressed =
                tag_picker_enter_allowed && ctx.input(|i| i.key_pressed(egui::Key::Escape));
            if right_panel_visible && let Some(metadata) = video_metadata.as_ref() {
                draw_native_metadata_panel(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    metadata,
                    &mut self.tag_picker_open,
                    &mut self.tag_picker_input,
                    &mut self.tag_picker_focus_request,
                    &mut self.tag_panel_sticky_item_key,
                    &mut self.tag_panel_sticky_tags,
                    &mut self.tag_picker_recent_tab,
                    tag_picker_enter_pressed,
                    tag_picker_escape_pressed,
                    &mut commands,
                    self.side_panel_mode.normalized() == FsSidePanelMode::ClickToShow,
                );
            }
            if jump_panel_visible {
                let close_left = draw_native_video_left_panel(
                    ctx,
                    overlay_height_points,
                    position_secs,
                    &jump_entries,
                    &jump_texture_ids,
                    shortcut_labels,
                    &mut bookmark_title_edit,
                    &mut bulk_bookmark_dialog,
                    &mut video_left_panel_tab,
                    &mut video_adjustment_tab,
                    &mut video_adjustments,
                    &mut video_scale_filter,
                    &mut video_downscale_smoothing_percent,
                    video_scale_outside_note.as_deref(),
                    &mut video_anime4k_budget,
                    &video_anime4k_status,
                    &video_preset_slots,
                    &creative_lut_choices,
                    &mut commands,
                    self.side_panel_mode.normalized() == FsSidePanelMode::ClickToShow,
                );
                if close_left {
                    left_panel_open = crate::ui_helpers::MetadataPanelOpenState::Closed;
                }
            }
            // Panel より後に描いて、開いている panel の端でも callout をクリック可能にする。
            if left_callout_visible
                && draw_native_panel_callout(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    true,
                    left_panel_open.is_open(),
                )
            {
                left_panel_open = crate::ui_helpers::toggle_side_panel_by_pointer(
                    self.side_panel_mode,
                    left_panel_open,
                );
            }
            if right_callout_visible
                && draw_native_panel_callout(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    false,
                    crate::ui_helpers::metadata_panel_explicit_shown(
                        self.side_panel_mode,
                        self.info_panel_open,
                    ),
                )
            {
                self.tag_picker_open = false;
                commands.push(NativeOverlayCommand::ToggleClickInfoOpen);
            }
            // Touch handles are separate widgets from the unchanged mouse
            // callouts. Their rects are also published to the HUD HWND region,
            // so HUD-source widget passthrough completes these as normal clicks.
            if let Some(rect) = touch_panel_handle_rects[0]
                && draw_native_touch_panel_handle(ctx, rect, true)
            {
                left_panel_open = crate::ui_helpers::open_side_panel_by_touch(left_panel_open);
            }
            if let Some(rect) = touch_panel_handle_rects[1]
                && draw_native_touch_panel_handle(ctx, rect, false)
            {
                commands.push(NativeOverlayCommand::OpenTouchInfoPanel);
            }
            if bookmark_title_edit.is_some() {
                last_drawn_bookmark_editor_rect = draw_native_bookmark_title_editor(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    &mut bookmark_title_edit,
                    &mut commands,
                );
            }
            if bulk_bookmark_dialog.is_some() {
                last_drawn_bulk_bookmark_dialog_rect = draw_native_bulk_bookmark_dialog(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    "動画",
                    &mut bulk_bookmark_dialog,
                    &mut commands,
                );
            }
            if shortcut_help_open {
                if let Some(metadata) = video_metadata.as_ref() {
                    last_drawn_shortcut_help_rect = draw_native_shortcut_help_dialog(
                        ctx,
                        overlay_width_points,
                        overlay_height_points,
                        metadata.shortcut_help.as_ref(),
                        !audio_only,
                        &mut shortcut_help_open,
                    );
                } else {
                    shortcut_help_open = false;
                }
            }
            // Codex 4周目 P1: 旧コードは `if !bottom_hud_visible { return; }` で
            // 早期 return していたが、それだと bottom HUD 非表示時に下の
            // `draw_native_normalize_progress` まで届かない (= スキャン中に HUD が
            // フェードアウトすると進捗 UI も消える)。bottom HUD だけを条件分岐し、
            // progress UI 描画は必ず実行する。
            // 「現在再生中のチャプター/ブックマーク」バナー (= P10-2)。
            // ジャンプパネル (= 左パネル) が表示されている間だけ、シークバー直上に
            // 直近マーカーのタイトルを 1 行で出す。
            //
            // 「直近」= 現在再生位置の手前にある最後のマーカー (Chapter / Bookmark)。
            // Pin は除外 (= フレームピンであって「再生中の区間」を表さないため)。
            // 種別優先順位は無し: pts_secs が最大のものを採用 (= 直近の方を表示)。
            // 詳細は `find_now_playing_marker` の doc を参照。
            if bottom_hud_visible
                && jump_panel_visible_for_banner
                && let Some(now_marker) =
                    super::overlay_draw::find_now_playing_marker(
                        &jump_entries,
                        position_secs,
                    )
            {
                let (kind_label, kind_color) = match now_marker.kind {
                    NativeOverlayTimelineMarkerKind::Bookmark => {
                        ("BM", egui::Color32::from_rgb(255, 220, 82))
                    }
                    NativeOverlayTimelineMarkerKind::Chapter => {
                        ("CH", egui::Color32::from_rgb(115, 210, 255))
                    }
                    NativeOverlayTimelineMarkerKind::Pin => {
                        // find_now_playing_marker が Pin を弾くのでここには来ない。
                        // 防御的に PIN 色を使う。
                        ("PIN", egui::Color32::from_rgb(140, 245, 170))
                    }
                };
                let title_text = now_marker
                    .title
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| match now_marker.kind {
                        NativeOverlayTimelineMarkerKind::Bookmark => "(無題)".to_string(),
                        NativeOverlayTimelineMarkerKind::Chapter => "(無題)".to_string(),
                        _ => "(無題)".to_string(),
                    });
                let banner_height = 26.0;
                let banner_gap = 6.0;
                let banner_y =
                    (overlay_height_points - HUD_BOTTOM_HEIGHT - banner_height - banner_gap)
                        .max(0.0);
                // 左パネル幅の右端から 12pt 右、シークバー直上に配置。
                // ユーザー案の図 (左パネル + 動画 + シークバー上にバナー) に合わせる。
                let banner_x = native_jump_panel_width() + 12.0;
                egui::Area::new(egui::Id::new("native_video_now_playing_banner"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(egui::pos2(banner_x, banner_y))
                    .interactable(false)
                    .show(ctx, |ui| {
                        egui::Frame::popup(ui.style())
                            .fill(egui::Color32::from_rgba_premultiplied(0, 0, 0, 200))
                            .stroke(egui::Stroke::NONE)
                            .corner_radius(4.0)
                            .inner_margin(egui::Margin::symmetric(8, 4))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 6.0;
                                    ui.label(
                                        egui::RichText::new(kind_label)
                                            .size(11.0)
                                            .strong()
                                            .color(kind_color),
                                    );
                                    ui.label(
                                        egui::RichText::new(title_text)
                                            .size(13.0)
                                            .color(egui::Color32::from_rgb(230, 230, 230)),
                                    );
                                });
                            });
                    });
            }

            if let Some(strip) = seek_strip.as_ref() {
                if bottom_hud_visible {
                    seek_strip_area_ran = true;
                    seek_strip_preview_hover = draw_native_seek_strip(
                        ctx,
                        egui::vec2(overlay_width_points, overlay_height_points),
                        visibility_hover_pos,
                        !bookmark_title_edit_visible
                            && !bulk_bookmark_dialog_visible
                            && !shortcut_help_open,
                        strip,
                        bottom_lock.strip_locked(),
                        &seek_strip_texture_ids,
                        seek_strip_wave_texture_id,
                        &mut seek_strip_drag_origin,
                        &mut last_seek_strip_window_request,
                        &mut commands,
                    );
                } else {
                    commands.push(NativeOverlayCommand::CloseSeekStrip {
                        cause: crate::video::seek_strip::SeekStripCloseCause::HudHidden,
                    });
                }
            }

            if bottom_hud_visible {
                egui::Area::new(egui::Id::new("native_video_seek_hud"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(egui::pos2(
                        0.0,
                        (overlay_height_points - HUD_BOTTOM_HEIGHT).max(0.0),
                    ))
                    .show(ctx, |ui| {
                        ui.set_min_size(egui::vec2(overlay_width_points, HUD_BOTTOM_HEIGHT));
                        let hud_rect = ui.min_rect();
                        let painter = ui.painter().clone();
                        let painter = &painter;
                        painter.rect_filled(
                            hud_rect,
                            0.0,
                            egui::Color32::from_rgba_premultiplied(0, 0, 0, 176),
                        );

                        // - シーク行 (上段、`HUD_SEEK_ROW_HEIGHT` = 24pt): bar + hover サムネ trigger
                        // - コントロール行 (下段、`HUD_CONTROLS_ROW_HEIGHT` = 40pt): ボタン群 + 音量
                        // `center_y` はコントロール行内の縦中央 (= ボタン群の Y 基準) として使う。
                        // 旧 1 段構造の bar Y 共有から外し、bar は seek_row_rect 内に独立配置する。
                        let seek_row_rect = egui::Rect::from_min_max(
                            hud_rect.min,
                            egui::pos2(hud_rect.max.x, hud_rect.min.y + HUD_SEEK_ROW_HEIGHT),
                        );
                        let controls_row_rect = egui::Rect::from_min_max(
                            egui::pos2(hud_rect.min.x, seek_row_rect.max.y),
                            hud_rect.max,
                        );

                        let side_pad = 10.0;
                        let btn_size = 28.0;
                        let gap = 8.0;
                        // 動画 HUD 2 段化リデザイン (実機フィードバック反映): ボタン群の意味的境界に
                        // **追加の隙間** (= group_gap_extra) を入れて、4 グループ
                        // [W][▶] | [L][⤴][↑][↓] | [|◀M][M▶|] | [◀F][📋][💾][F▶]
                        // を視覚的に分離する。隣接ボタン間は通常 `gap` (= 8pt)、グループ境界では
                        // `gap + group_gap_extra` (= 16pt 相当) の間隔を取る。
                        let group_gap_extra = 8.0;
                        let center_y = controls_row_rect.center().y;
                        let text_center_y = center_y + 4.0;

                        // 動画 HUD 2 段化リデザイン (実機フィードバック反映: 左側ボタン優先):
                        // **左にあるボタンほど残す** という優先順位で compaction tier を決める。
                        // ユーザー指摘: 旧版はキャプチャパレットを最後まで残していたが、
                        // 前/次ファイル移動のほうが使用頻度が高い。レイアウトの直感性も
                        // 「左から順に消える」のほうが分かりやすい。
                        //
                        //   tier 0 (Full)       : 全ボタン + フル右クラスター
                        //   tier 1 (NoCapture)  : キャプチャパレット (4 ボタン) を一括非表示
                        //   tier 2 (NoMarkers)  : tier 1 + マーカー (J/K) も非表示
                        //   tier 3 (NoFileNav)  : tier 2 + 前/次項目 (↑/↓) も非表示
                        //                         → 左側 4 ボタン (W/▶/L/⤴) のみ、右クラスター full
                        //   tier 4 (Minimal)    : tier 3 + 右クラスター縮小 (vol_slider 100pt
                        //                         + 音量ラベル / リミッター非表示)。最小窓 640pt 対応。
                        //
                        // tier 閾値は左/右クラスター幅の実数値から導出。各 tier に該当する
                        // フラグ (show_capture_palette / show_markers / show_file_nav /
                        // compact_right_cluster) で個別ボタンを gate する。
                        let time_w = 132.0;
                        let vol_label_w = 60.0;
                        let limiter_indicator_w = 14.0;
                        let vol_slider_w_full = 144.0;
                        let vol_slider_w_narrow = 100.0;
                        let mute_w = btn_size;
                        let norm_w = btn_size;
                        let speed_w = btn_size * 1.55;
                        let lock_w = btn_size;
                        let seek_strip_selector_reservation = if audio_only {
                            0.0
                        } else {
                            btn_size + gap
                        };
                        let right_w_full = time_w
                            + gap
                            + speed_w
                            + gap
                            + mute_w
                            + gap
                            + norm_w
                            + gap
                            + vol_slider_w_full
                            + gap
                            + vol_label_w
                            + limiter_indicator_w
                            + seek_strip_selector_reservation
                            + gap
                            + lock_w;
                        let right_w_compact = time_w
                            + gap
                            + speed_w
                            + gap
                            + mute_w
                            + gap
                            + norm_w
                            + gap
                            + vol_slider_w_narrow
                            + seek_strip_selector_reservation
                            + gap
                            + lock_w;
                        // 左クラスター幅: 各ボタンを btn_size、ボタン間 gap、グループ境界に group_gap_extra
                        // 各 tier での想定幅 (= 左ボタン群、side_pad は別途加算)。
                        // 数値は下記のボタン描画ループと厳密一致させること。
                        let group_a = btn_size + gap + btn_size; // W, play
                        let group_b_full =
                            btn_size + gap + btn_size + gap + btn_size + gap + btn_size; // L, cont, ↑, ↓
                        let group_b_compact = btn_size + gap + btn_size; // L, cont
                        let group_c = btn_size + gap + btn_size; // prev_M, next_M
                        let group_d = btn_size + gap + btn_size + gap + btn_size + gap + btn_size; // capture palette
                        let group_boundary = gap + group_gap_extra; // A|B, B|C, C|D 共通
                        let left_w_full = group_a
                            + group_boundary
                            + group_b_full
                            + group_boundary
                            + group_c
                            + group_boundary
                            + group_d;
                        let left_w_no_capture =
                            group_a + group_boundary + group_b_full + group_boundary + group_c;
                        let left_w_no_markers = group_a + group_boundary + group_b_full;
                        let left_w_no_file_nav = group_a + group_boundary + group_b_compact;

                        let total_full = side_pad * 2.0 + left_w_full + gap + right_w_full;
                        let total_no_capture =
                            side_pad * 2.0 + left_w_no_capture + gap + right_w_full;
                        let total_no_markers =
                            side_pad * 2.0 + left_w_no_markers + gap + right_w_full;
                        let total_no_file_nav =
                            side_pad * 2.0 + left_w_no_file_nav + gap + right_w_full;
                        // tier 4 (Minimal) は left = left_w_no_file_nav、right = right_w_compact
                        // で約 571pt 以上で収まる (= 最小窓 640pt 対応)。

                        #[derive(Copy, Clone, PartialEq)]
                        enum CompactionTier {
                            Full,
                            NoCapture,
                            NoMarkers,
                            NoFileNav,
                            Minimal,
                        }
                        let tier = if overlay_width_points >= total_full {
                            CompactionTier::Full
                        } else if overlay_width_points >= total_no_capture {
                            CompactionTier::NoCapture
                        } else if overlay_width_points >= total_no_markers {
                            CompactionTier::NoMarkers
                        } else if overlay_width_points >= total_no_file_nav {
                            CompactionTier::NoFileNav
                        } else {
                            CompactionTier::Minimal
                        };
                        let show_capture_palette = matches!(tier, CompactionTier::Full);
                        let show_markers =
                            matches!(tier, CompactionTier::Full | CompactionTier::NoCapture);
                        let show_file_nav = matches!(
                            tier,
                            CompactionTier::Full
                                | CompactionTier::NoCapture
                                | CompactionTier::NoMarkers
                        );
                        let compact_right_cluster = matches!(tier, CompactionTier::Minimal);

                        let mut x = hud_rect.min.x + side_pad;

                        let replay_rect = egui::Rect::from_min_size(
                            egui::pos2(x, center_y - btn_size * 0.5),
                            egui::vec2(btn_size, btn_size),
                        );
                        let replay_resp = ui.interact(
                            replay_rect,
                            egui::Id::new("native_video_replay"),
                            egui::Sense::click(),
                        );
                        draw_overlay_button_bg(painter, replay_rect, replay_resp.hovered(), false);
                        draw_overlay_replay_icon(painter, replay_rect.center(), btn_size * 0.36);
                        let replay_resp = replay_resp.hover_tip_dark(native_label_with_shortcut(
                            "最初から再生 (頭出し + 即再生)",
                            shortcut_labels.and_then(|s| s.seek_start.as_deref()),
                        ));
                        if replay_resp.clicked() {
                            commands.push(NativeOverlayCommand::SeekToStartAndPlay);
                        }
                        x = replay_rect.max.x + gap;

                        let play_rect = egui::Rect::from_min_size(
                            egui::pos2(x, center_y - btn_size * 0.5),
                            egui::vec2(btn_size, btn_size),
                        );
                        let play_resp = ui.interact(
                            play_rect,
                            egui::Id::new("native_video_play"),
                            egui::Sense::click(),
                        );
                        draw_overlay_button_bg(painter, play_rect, play_resp.hovered(), false);
                        if is_playing {
                            draw_overlay_pause_icon(painter, play_rect.center(), btn_size * 0.30);
                        } else {
                            draw_overlay_play_icon(painter, play_rect.center(), btn_size * 0.38);
                        }
                        let play_resp = play_resp.hover_tip_dark(native_label_with_shortcut(
                            if is_playing {
                                "一時停止"
                            } else {
                                "再生"
                            },
                            shortcut_labels.and_then(|s| s.play_pause.as_deref()),
                        ));
                        if play_resp.clicked() {
                            commands.push(NativeOverlayCommand::TogglePlay);
                        }
                        // グループ境界: [W][▶] | [L][⤴][↑][↓]
                        x = play_rect.max.x + gap + group_gap_extra;

                        let loop_rect = egui::Rect::from_min_size(
                            egui::pos2(x, center_y - btn_size * 0.5),
                            egui::vec2(btn_size, btn_size),
                        );
                        let loop_resp = ui.interact(
                            loop_rect,
                            egui::Id::new("native_video_loop"),
                            egui::Sense::click(),
                        );
                        // 4 段階ループモード: Off / Full / Chapter / Bookmark
                        // - Off: アイコン中央、active 背景なし
                        // - Full: アイコン中央、active 背景 (青)
                        // - Chapter: アイコン下半分縮小 + 上に「CH」テキスト
                        // - Bookmark: アイコン下半分縮小 + 上にブックマーク vector アイコン
                        // active 背景は loop_mode != Off (= 表示用) に基づき判定する。
                        // 「BM 設定 + BM 無し動画」では loop_enabled は true (Full と等価) でも
                        // ボタン表示は BM のまま (active 背景 + BM 装飾) になる。
                        use crate::settings::VideoLoopMode;
                        let continuous_active = continuous_mode.is_enabled();
                        let mode_active =
                            !continuous_active && !matches!(loop_mode, VideoLoopMode::Off);
                        draw_overlay_button_bg(
                            painter,
                            loop_rect,
                            loop_resp.hovered() && !continuous_active,
                            mode_active,
                        );
                        let icon_color = if continuous_active {
                            egui::Color32::from_gray(120)
                        } else if mode_active {
                            egui::Color32::from_rgb(170, 230, 255)
                        } else {
                            egui::Color32::from_rgb(238, 238, 238)
                        };
                        match loop_mode {
                            VideoLoopMode::Off | VideoLoopMode::Full => {
                                draw_overlay_loop_icon(
                                    painter,
                                    loop_rect.center(),
                                    btn_size * 0.36,
                                    icon_color,
                                );
                            }
                            VideoLoopMode::Chapter => {
                                let r = btn_size * 0.36;
                                let c = loop_rect.center();
                                draw_overlay_loop_icon(
                                    painter,
                                    egui::pos2(c.x, c.y + r * 0.18),
                                    r * 0.65,
                                    icon_color,
                                );
                                painter.text(
                                    egui::pos2(c.x, c.y - r * 0.55),
                                    egui::Align2::CENTER_CENTER,
                                    "CH",
                                    crate::ui_fonts::hud_text_font(11.0),
                                    egui::Color32::from_rgb(115, 210, 255),
                                );
                            }
                            VideoLoopMode::Bookmark => {
                                let r = btn_size * 0.36;
                                let c = loop_rect.center();
                                draw_overlay_loop_icon(
                                    painter,
                                    egui::pos2(c.x, c.y + r * 0.18),
                                    r * 0.65,
                                    icon_color,
                                );
                                draw_overlay_bookmark_icon(
                                    painter,
                                    egui::pos2(c.x, c.y - r * 0.55),
                                    r * 0.32,
                                    egui::Color32::from_rgb(255, 220, 82),
                                );
                            }
                        }
                        let hover_text = if continuous_active {
                            "連続再生中はループ無効".to_owned()
                        } else {
                            native_label_with_shortcut(match loop_mode {
                                VideoLoopMode::Off => "ループ再生",
                                VideoLoopMode::Full => "ループ: 全体",
                                VideoLoopMode::Chapter => "ループ: チャプター",
                                VideoLoopMode::Bookmark => "ループ: ブックマーク",
                            }, shortcut_labels.and_then(|s| s.loop_mode.as_deref()))
                        };
                        let loop_resp = loop_resp.hover_tip_dark(hover_text);
                        if loop_resp.clicked() && !continuous_active {
                            commands.push(NativeOverlayCommand::ToggleLoop);
                        }
                        let _ = loop_enabled; // mode_active ベース描画なので未使用
                        x = loop_rect.max.x + gap;

                        let continuous_rect = egui::Rect::from_min_size(
                            egui::pos2(x, center_y - btn_size * 0.5),
                            egui::vec2(btn_size, btn_size),
                        );
                        let continuous_resp = ui.interact(
                            continuous_rect,
                            egui::Id::new("native_video_continuous"),
                            egui::Sense::click(),
                        );
                        draw_overlay_button_bg(
                            painter,
                            continuous_rect,
                            continuous_resp.hovered(),
                            continuous_active,
                        );
                        draw_overlay_continuous_icon(painter, continuous_rect, continuous_mode);
                        let continuous_hover = match continuous_mode {
                            crate::video::VideoContinuousMode::Off => "連続再生",
                            crate::video::VideoContinuousMode::Continuous => "連続再生: 末尾で停止",
                            crate::video::VideoContinuousMode::ContinuousLoop => {
                                "連続再生: 末尾で先頭へ"
                            }
                        };
                        let continuous_resp = continuous_resp.hover_tip_dark(continuous_hover);
                        if continuous_resp.clicked() {
                            commands.push(NativeOverlayCommand::ToggleContinuous);
                        }
                        // 動画 HUD 2 段化リデザイン (実機フィードバック反映): file_nav が省略された
                        // ときは continuous がここで group B の末尾になる。後ろに何か続くなら
                        // (marker または capture)、group_gap_extra を入れて group 境界を視覚化する。
                        x = continuous_rect.max.x + gap;
                        if !show_file_nav && (show_markers || show_capture_palette) {
                            x += group_gap_extra;
                        }

                        // 動画 HUD 2 段化リデザイン (Phase 6): 前/次ファイル (前/次項目) ボタン。
                        // ↑/↓ キー / マウスホイールと同じ NavigateItem コマンドを送出する
                        // (= 既存の navigate_native_video_fullscreen 経由、境界では EOF
                        // トーストが自動で出る)。連続再生 / ループ の隣に配置することで
                        // 「左右=動画内、上下=ファイル切替」の規約と「連続再生=末尾で次へ」の
                        // 意味的隣接を視覚化する。
                        // 狭幅ウィンドウ (NoFileNav 以上) では非表示にして右クラスター
                        // (時間 / 音量) との overlap を避ける (キーボード ↑↓ で代替可能)。
                        if show_file_nav {
                            let prev_file_rect = egui::Rect::from_min_size(
                                egui::pos2(x, center_y - btn_size * 0.5),
                                egui::vec2(btn_size, btn_size),
                            );
                            let prev_file_resp = ui.interact(
                                prev_file_rect,
                                egui::Id::new("native_video_prev_file"),
                                egui::Sense::click(),
                            );
                            draw_overlay_button_bg(
                                painter,
                                prev_file_rect,
                                prev_file_resp.hovered(),
                                false,
                            );
                            draw_overlay_arrow_icon(painter, prev_file_rect, -1);
                            let prev_file_resp =
                                prev_file_resp.hover_tip_dark(native_label_with_shortcut(
                                    "前の項目",
                                    shortcut_labels.and_then(|s| s.prev_file.as_deref()),
                                ));
                            if prev_file_resp.clicked() {
                                commands.push(NativeOverlayCommand::NavigateItem {
                                    delta: -1,
                                    via_wheel: false,
                                });
                            }
                            x = prev_file_rect.max.x + gap;

                            let next_file_rect = egui::Rect::from_min_size(
                                egui::pos2(x, center_y - btn_size * 0.5),
                                egui::vec2(btn_size, btn_size),
                            );
                            let next_file_resp = ui.interact(
                                next_file_rect,
                                egui::Id::new("native_video_next_file"),
                                egui::Sense::click(),
                            );
                            draw_overlay_button_bg(
                                painter,
                                next_file_rect,
                                next_file_resp.hovered(),
                                false,
                            );
                            draw_overlay_arrow_icon(painter, next_file_rect, 1);
                            let next_file_resp =
                                next_file_resp.hover_tip_dark(native_label_with_shortcut(
                                    "次の項目",
                                    shortcut_labels.and_then(|s| s.next_file.as_deref()),
                                ));
                            if next_file_resp.clicked() {
                                commands.push(NativeOverlayCommand::NavigateItem {
                                    delta: 1,
                                    via_wheel: false,
                                });
                            }
                            // グループ境界: [L][⤴][↑][↓] | (次にマーカー or キャプチャ がある場合のみ)
                            x = next_file_rect.max.x + gap;
                            if show_markers || show_capture_palette {
                                x += group_gap_extra;
                            }
                        } // ← `if show_file_nav` の閉じ (前/次項目ブロック)

                        // 動画 HUD 2 段化リデザイン (Phase 4): 前/次マーカーボタン
                        // (chapter / bookmark / pin)。J / K キーと同じ
                        // `jump_native_video_marker` を呼ぶ。マーカー 0 個では disabled
                        // (= 非表示ではなくグレーアウト、レイアウト揺れを避けるため)。
                        // 狭幅ウィンドウ (NoMarkers 以上) では非表示
                        // (キーボード J/K で代替可能)。
                        let markers_present = !timeline_markers.is_empty();
                        if show_markers {
                            let prev_marker_rect = egui::Rect::from_min_size(
                                egui::pos2(x, center_y - btn_size * 0.5),
                                egui::vec2(btn_size, btn_size),
                            );
                            let prev_marker_resp = if markers_present {
                                ui.interact(
                                    prev_marker_rect,
                                    egui::Id::new("native_video_prev_marker"),
                                    egui::Sense::click(),
                                )
                            } else {
                                ui.interact(
                                    prev_marker_rect,
                                    egui::Id::new("native_video_prev_marker"),
                                    egui::Sense::hover(),
                                )
                            };
                            draw_overlay_button_bg(
                                painter,
                                prev_marker_rect,
                                prev_marker_resp.hovered() && markers_present,
                                false,
                            );
                            draw_overlay_skip_to_marker_icon(
                                painter,
                                prev_marker_rect,
                                -1,
                                markers_present,
                            );
                            let prev_marker_resp = prev_marker_resp.hover_tip_dark(
                                if markers_present {
                                    native_label_with_shortcut(
                                        "前のマーカー (チャプター/ブックマーク/ピン)",
                                        shortcut_labels.and_then(|s| s.marker_prev.as_deref()),
                                    )
                                } else {
                                    "マーカーがありません".to_owned()
                                },
                            );
                            if markers_present && prev_marker_resp.clicked() {
                                commands.push(NativeOverlayCommand::JumpMarker { next: false });
                            }
                            x = prev_marker_rect.max.x + gap;

                            let next_marker_rect = egui::Rect::from_min_size(
                                egui::pos2(x, center_y - btn_size * 0.5),
                                egui::vec2(btn_size, btn_size),
                            );
                            let next_marker_resp = if markers_present {
                                ui.interact(
                                    next_marker_rect,
                                    egui::Id::new("native_video_next_marker"),
                                    egui::Sense::click(),
                                )
                            } else {
                                ui.interact(
                                    next_marker_rect,
                                    egui::Id::new("native_video_next_marker"),
                                    egui::Sense::hover(),
                                )
                            };
                            draw_overlay_button_bg(
                                painter,
                                next_marker_rect,
                                next_marker_resp.hovered() && markers_present,
                                false,
                            );
                            draw_overlay_skip_to_marker_icon(
                                painter,
                                next_marker_rect,
                                1,
                                markers_present,
                            );
                            let next_marker_resp = next_marker_resp.hover_tip_dark(
                                if markers_present {
                                    native_label_with_shortcut(
                                        "次のマーカー (チャプター/ブックマーク/ピン)",
                                        shortcut_labels.and_then(|s| s.marker_next.as_deref()),
                                    )
                                } else {
                                    "マーカーがありません".to_owned()
                                },
                            );
                            if markers_present && next_marker_resp.clicked() {
                                commands.push(NativeOverlayCommand::JumpMarker { next: true });
                            }
                            // グループ境界: [|◀M][M▶|] | (キャプチャがある場合のみ)
                            x = next_marker_rect.max.x + gap;
                            if show_capture_palette {
                                x += group_gap_extra;
                            }
                        } // ← `if show_markers` の閉じ (マーカーブロック)

                        // 動画 HUD 2 段化リデザイン (実機フィードバック反映: 左側優先):
                        // キャプチャパレット (前フレーム / コピー / 保存 / 次フレーム) は
                        // **キーボードに代替経路がある** ボタン群なので、狭幅では一括非表示にする
                        // (旧版はカメラだけ残していたが、ユーザー優先度では前/次項目のほうが
                        // 重要なため camera-only 中間 tier を廃止)。
                        // 代替経路: Ctrl+S (保存) / Ctrl+Shift+←/→ (フレームステップ) /
                        // ホバーバーのキャプチャアイコンは将来追加余地あり。
                        let mut prev_down = false;
                        let mut next_down = false;
                        if show_capture_palette {
                            let prev_frame_rect = egui::Rect::from_min_size(
                                egui::pos2(x, center_y - btn_size * 0.5),
                                egui::vec2(btn_size, btn_size),
                            );
                            prev_down = draw_native_frame_step_button(
                                ui,
                                painter,
                                prev_frame_rect,
                                "native_video_prev_frame",
                                -1,
                                "前のフレーム [Ctrl+Shift+←]",
                                &mut frame_step_hold,
                                &mut commands,
                            );
                            x = prev_frame_rect.max.x + gap;

                            let screenshot_rect = egui::Rect::from_min_size(
                                egui::pos2(x, center_y - btn_size * 0.5),
                                egui::vec2(btn_size, btn_size),
                            );
                            let screenshot_resp = ui.interact(
                                screenshot_rect,
                                egui::Id::new("native_video_screenshot"),
                                egui::Sense::click(),
                            );
                            draw_overlay_button_bg(
                                painter,
                                screenshot_rect,
                                screenshot_resp.hovered(),
                                false,
                            );
                            draw_overlay_camera_icon(painter, screenshot_rect);
                            let screenshot_resp = screenshot_resp
                                .hover_tip_dark("現在フレームをクリップボードにコピー");
                            if screenshot_resp.clicked() {
                                commands.push(NativeOverlayCommand::CopyFrameToClipboard);
                            }
                            x = screenshot_rect.max.x + gap;

                            let save_rect = egui::Rect::from_min_size(
                                egui::pos2(x, center_y - btn_size * 0.5),
                                egui::vec2(btn_size, btn_size),
                            );
                            let save_resp = ui.interact(
                                save_rect,
                                egui::Id::new("native_video_save_frame"),
                                egui::Sense::click(),
                            );
                            draw_overlay_button_bg(painter, save_rect, save_resp.hovered(), false);
                            draw_overlay_save_icon(painter, save_rect);
                            let save_resp =
                                save_resp.hover_tip_dark(native_label_with_shortcut(
                                    "現在フレームをファイル保存",
                                    shortcut_labels.and_then(|s| s.capture.as_deref()),
                                ));
                            if save_resp.clicked() {
                                commands.push(NativeOverlayCommand::SaveFrameToFile);
                            }
                            x = save_rect.max.x + gap;

                            let next_frame_rect = egui::Rect::from_min_size(
                                egui::pos2(x, center_y - btn_size * 0.5),
                                egui::vec2(btn_size, btn_size),
                            );
                            let next_inner_down = draw_native_frame_step_button(
                                ui,
                                painter,
                                next_frame_rect,
                                "native_video_next_frame",
                                1,
                                "次のフレーム [Ctrl+Shift+→]",
                                &mut frame_step_hold,
                                &mut commands,
                            );
                            next_down = next_inner_down;
                            // bar はシーク行に独立配置するため、末尾の x 更新は不要 (= 旧 1 段で
                            // bar_min_x = x として残空間を bar に割り当てていた名残)。
                        }
                        if !prev_down && !next_down {
                            frame_step_hold = None;
                        }

                        // 動画 HUD 2 段化リデザイン (実機フィードバック反映): 右クラスター幅は
                        // 上で tier 判定済みの `compact_right_cluster` に応じて伸縮する。
                        // 最小窓 (640pt) では vol_label / limiter 非表示 + vol_slider 縮小で
                        // 左ボタン群との overlap を回避する。
                        let vol_slider_w = if compact_right_cluster {
                            vol_slider_w_narrow
                        } else {
                            vol_slider_w_full
                        };
                        let show_vol_label = !compact_right_cluster;
                        let show_limiter_slot = !compact_right_cluster;
                        let right_controls_w = if compact_right_cluster {
                            right_w_compact
                        } else {
                            right_w_full
                        };
                        let right_controls_x = hud_rect.max.x - side_pad - right_controls_w;
                        // 動画 HUD 2 段化リデザイン (Phase 3): bar はシーク行 (上段) に独立配置し、
                        // **フル幅** で seek_row_rect の左右 padding 内に展開する。コントロール行
                        // (下段) からは bar が消えるので、ボタンと時間表示の間は空きスペースになる。
                        // hit_rect は seek_row_rect 全体 (= 24pt) を覆い、bar のヒット領域を厚く取る。
                        let bar_min_x = seek_row_rect.min.x + side_pad;
                        let bar_max_x = (seek_row_rect.max.x - side_pad).max(bar_min_x + 1.0);
                        let bar_center_y = seek_row_rect.center().y;
                        let bar_rect = egui::Rect::from_min_max(
                            egui::pos2(bar_min_x, bar_center_y - 4.0),
                            egui::pos2(bar_max_x, bar_center_y + 4.0),
                        );
                        let hit_rect = egui::Rect::from_min_max(
                            egui::pos2(bar_min_x, seek_row_rect.min.y),
                            egui::pos2(bar_max_x, seek_row_rect.max.y),
                        );

                        // time は right_controls クラスターの左端 (= 旧レイアウトと同じ位置)。
                        // right_controls_w に time_w が含まれているため、`right_controls_x` がそのまま
                        // time の左端になる。controls 行の左ボタン群との間は空きを残す (シーク行の
                        // bar と視覚的に対応)。
                        let time_x = right_controls_x;
                        let label = format!(
                            "{} / {}",
                            format_overlay_time(position_secs),
                            format_overlay_time(duration_secs)
                        );
                        painter.text(
                            egui::pos2(time_x, text_center_y),
                            egui::Align2::LEFT_CENTER,
                            label,
                            crate::ui_fonts::hud_text_font(14.0),
                            egui::Color32::from_rgb(238, 238, 238),
                        );

                        let speed_rect = egui::Rect::from_min_size(
                            egui::pos2(time_x + time_w + gap, center_y - btn_size * 0.5),
                            egui::vec2(speed_w, btn_size),
                        );
                        let mute_rect = egui::Rect::from_min_size(
                            egui::pos2(speed_rect.max.x + gap, center_y - btn_size * 0.5),
                            egui::vec2(mute_w, btn_size),
                        );
                        let norm_rect = egui::Rect::from_min_size(
                            egui::pos2(mute_rect.max.x + gap, center_y - btn_size * 0.5),
                            egui::vec2(norm_w, btn_size),
                        );
                        let vol_rect = egui::Rect::from_min_max(
                            egui::pos2(norm_rect.max.x + gap, center_y - 4.0),
                            egui::pos2(norm_rect.max.x + gap + vol_slider_w, center_y + 4.0),
                        );
                        let seek_lock_rect = native_seek_bar_lock_button_rect(
                            overlay_width_points,
                            overlay_height_points,
                        );
                        let seek_strip_selector_rect = native_seek_strip_selector_button_rect(
                            overlay_width_points,
                            overlay_height_points,
                        );

                        painter.rect_filled(bar_rect, 2.0, egui::Color32::from_gray(74));
                        if position_controls_available {
                            let progress = (position_secs / duration_secs).clamp(0.0, 1.0) as f32;
                            let filled = egui::Rect::from_min_max(
                                bar_rect.min,
                                egui::pos2(
                                    bar_rect.min.x + bar_rect.width() * progress,
                                    bar_rect.max.y,
                                ),
                            );
                            painter.rect_filled(
                                filled,
                                2.0,
                                egui::Color32::from_rgb(228, 228, 228),
                            );
                        }
                        for tick in crate::seek_ruler::duration_ruler_ticks(
                            duration_secs,
                            bar_rect.width(),
                            crate::seek_ruler::SEEK_RULER_TIME_MIN_SPACING,
                        ) {
                            let x = bar_rect.min.x + bar_rect.width() * tick.fraction;
                            let top = bar_rect.bottom() + crate::seek_ruler::SEEK_RULER_GAP;
                            let (height, gray) = if tick.major {
                                (
                                    crate::seek_ruler::SEEK_RULER_MAJOR_HEIGHT,
                                    crate::seek_ruler::SEEK_RULER_MAJOR_GRAY,
                                )
                            } else {
                                (
                                    crate::seek_ruler::SEEK_RULER_MINOR_HEIGHT,
                                    crate::seek_ruler::SEEK_RULER_MINOR_GRAY,
                                )
                            };
                            painter.line_segment(
                                [egui::pos2(x, top), egui::pos2(x, top + height)],
                                egui::Stroke::new(
                                    crate::seek_ruler::SEEK_RULER_STROKE_WIDTH,
                                    egui::Color32::from_gray(gray),
                                ),
                            );
                        }
                        if position_controls_available {
                            for marker in &timeline_markers {
                                draw_timeline_marker(painter, bar_rect, duration_secs, *marker);
                            }
                        }

                        let seek_resp = ui.interact(
                            hit_rect,
                            egui::Id::new("native_video_seek_hit"),
                            if position_controls_available {
                                egui::Sense::click_and_drag()
                            } else {
                                egui::Sense::hover()
                            },
                        );
                        if position_controls_available && seek_resp.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                        }
                        if position_controls_available {
                            let target_at = |pos: egui::Pos2| {
                                let x = pos.x.clamp(bar_rect.min.x, bar_rect.max.x);
                                let frac =
                                    ((x - bar_rect.min.x) / bar_rect.width()).clamp(0.0, 1.0);
                                duration_secs * frac as f64
                            };
                            if seek_resp.drag_started()
                                && let Some(origin) = seek_resp.interact_pointer_pos()
                            {
                                seek_row_gesture = Some(
                                    crate::video::seek_strip::SeekRowGesture::new(origin),
                                );
                            }
                            if seek_resp.clicked()
                                && let Some(pos) = seek_resp.interact_pointer_pos()
                            {
                                let target_secs = target_at(pos);
                                last_seek_target_secs = Some(target_secs);
                                commands.push(NativeOverlayCommand::Seek { target_secs });
                            } else if seek_resp.dragged()
                                && let Some(pos) = seek_resp.interact_pointer_pos()
                                && let Some(gesture) = seek_row_gesture.as_mut()
                            {
                                let was_open = matches!(
                                    gesture,
                                    crate::video::seek_strip::SeekRowGesture::OpenStrip { .. }
                                );
                                match gesture.update(pos) {
                                    crate::video::seek_strip::SeekRowDecision::OpenStrip => {
                                        if seek_strip_material_available
                                            && !seek_strip_visible
                                            && !was_open
                                        {
                                            last_seek_target_secs = None;
                                            commands.push(NativeOverlayCommand::OpenSeekStrip);
                                        }
                                    }
                                    crate::video::seek_strip::SeekRowDecision::Scrub => {
                                        let target_secs = target_at(pos);
                                        let should_emit = last_seek_target_secs
                                            .map(|previous| {
                                                (previous - target_secs).abs() >= 0.10
                                            })
                                            .unwrap_or(true);
                                        if should_emit {
                                            last_seek_target_secs = Some(target_secs);
                                            commands.push(NativeOverlayCommand::Seek {
                                                target_secs,
                                            });
                                        }
                                    }
                                    crate::video::seek_strip::SeekRowDecision::Undecided => {}
                                }
                            }
                            if seek_resp.drag_stopped() {
                                if matches!(
                                    seek_row_gesture,
                                    Some(crate::video::seek_strip::SeekRowGesture::Undecided { .. })
                                ) && let Some(pos) = seek_resp.interact_pointer_pos()
                                {
                                    commands.push(NativeOverlayCommand::Seek {
                                        target_secs: target_at(pos),
                                    });
                                }
                                seek_row_gesture = None;
                                last_seek_target_secs = None;
                            }
                        } else {
                            seek_row_gesture = None;
                            last_seek_target_secs = None;
                        }
                        // 速度 popup が開いているときは、popup 内のアイテムを選びに行く
                        // カーソル動線が seek_row を横切るため preview を抑止する。ストリップは
                        // 別の抑止理由ではなく、seek row と同じ単一 target の producer になる。
                        let seek_preview_allowed =
                            position_controls_available && !audio_only && !video_speed_popup_open;
                        let strip_preview_active = if seek_preview_allowed {
                            match seek_strip_preview_hover {
                                NativeSeekStripPreviewHover::Target(target) => {
                                    // ストリップは自分が指されている x をそのまま渡す。
                                    // 時刻からシークバー上の x を引き直すと、軸が違うので
                                    // ポインタから離れた場所へ出る。
                                    let anchor = pointer_pos
                                        .map(|pos| pos.x)
                                        .unwrap_or(overlay_width_points * 0.5);
                                    set_seek_preview_target(
                                        &mut hover_preview_target_secs,
                                        &mut hover_preview_anchor_x,
                                        Some((target.clamp(0.0, duration_secs), anchor)),
                                    );
                                    true
                                }
                                NativeSeekStripPreviewHover::Suppress => {
                                    set_seek_preview_target(
                                        &mut hover_preview_target_secs,
                                        &mut hover_preview_anchor_x,
                                        None,
                                    );
                                    false
                                }
                                NativeSeekStripPreviewHover::Outside => false,
                            }
                        } else {
                            set_seek_preview_target(
                                &mut hover_preview_target_secs,
                                &mut hover_preview_anchor_x,
                                None,
                            );
                            false
                        };
                        if seek_preview_allowed
                            && !strip_preview_active
                            && seek_resp.hovered()
                            && let Some(pos) = pointer_pos
                        {
                            let x = pos.x.clamp(bar_rect.min.x, bar_rect.max.x);
                            let frac =
                                ((x - bar_rect.min.x) / bar_rect.width()).clamp(0.0, 1.0);
                            set_seek_preview_target(
                                &mut hover_preview_target_secs,
                                &mut hover_preview_anchor_x,
                                Some((duration_secs * frac as f64, x)),
                            );
                        }
                        if position_controls_available
                            && let Some(target) = hover_preview_target_secs
                        {
                            // プレビューは「指している物の上」に出す。anchor が無い経路
                            // (状態復元など) だけ、従来どおりシークバー上の位置へ落とす。
                            let x = hover_preview_anchor_x.unwrap_or_else(|| {
                                let frac = (target / duration_secs).clamp(0.0, 1.0) as f32;
                                bar_rect.min.x + bar_rect.width() * frac
                            });
                            let hover_preview_bookmarked = target_has_marker(
                                &timeline_markers,
                                target,
                                duration_secs,
                                |kind| kind == NativeOverlayTimelineMarkerKind::Bookmark,
                            );
                            let strip_rect = seek_strip_visible.then(|| {
                                native_seek_strip_rect(
                                    overlay_width_points,
                                    overlay_height_points,
                                )
                            });
                            let preview_layout = native_seek_preview_layout(
                                overlay_width_points,
                                x,
                                hud_rect,
                                strip_rect,
                            );
                            let preview_rect = preview_layout.preview_rect;
                            let image_rect = preview_layout.image_rect;
                            let action_rect = preview_layout.action_rect;
                            // 動画 HUD 2 段化リデザイン (実機フィードバック反映 #1):
                            // corridor の下端は **seek_row (シーク行) 底辺** まで。
                            // 旧 1 段の名残で hud_rect.max.y まで伸ばすと、controls 行に
                            // カーソルを降ろした瞬間も「まだ corridor 内」と判定されて
                            // hover preview が居座り、下のボタン (frame step / camera / 音量
                            // 等) を押しに行くときに preview が被さってしまう。
                            // corridor を seek_row 内に限定することで、controls 行に
                            // カーソルが入った瞬間 preview を即座に隠す。
                            let preview_corridor_rect = egui::Rect::from_min_max(
                                egui::pos2(preview_rect.min.x - 8.0, preview_rect.max.y),
                                egui::pos2(preview_rect.max.x + 8.0, seek_row_rect.max.y),
                            );
                            let pointer_in_preview = pointer_pos.is_some_and(|pos| {
                                preview_rect.expand(8.0).contains(pos)
                                    || preview_corridor_rect.contains(pos)
                            });
                            if !seek_resp.hovered()
                                && !strip_preview_active
                                && !pointer_in_preview
                            {
                                set_seek_preview_target(
                                    &mut hover_preview_target_secs,
                                    &mut hover_preview_anchor_x,
                                    None,
                                );
                            } else {
                                // 実機修正 (2026-05-12 P1 #2): 実描画 rect を記録。
                                // `compute_hud_regions` が region 計算で読む。
                                last_drawn_preview_rect = Some(preview_rect);
                                let request_due = last_thumbnail_request_secs
                                    .map(|previous| (previous - target).abs() >= 0.25)
                                    .unwrap_or(true)
                                    || last_thumbnail_request_at
                                        .map(|last| last.elapsed() >= Duration::from_millis(250))
                                        .unwrap_or(true);
                                if request_due {
                                    last_thumbnail_request_secs = Some(target);
                                    last_thumbnail_request_at = Some(Instant::now());
                                    commands.push(NativeOverlayCommand::RequestSeekThumbnail {
                                        target_secs: target,
                                        bar_width_points: bar_rect.width() as f64,
                                        pixels_per_point: self.pixels_per_point as f64,
                                    });
                                }
                                // 動画 HUD 2 段化リデザイン (Phase 3): hover カーソル縦線は
                                // **シーク行内のみ** に描く (旧 1 段では HUD 全体に伸びていた)。
                                // 2 段化後に HUD 全体へ伸ばすとコントロール行のボタン上に線が
                                // 重なって視認性が下がる。
                                painter.line_segment(
                                    [
                                        egui::pos2(x, seek_row_rect.min.y + 4.0),
                                        egui::pos2(x, seek_row_rect.max.y - 4.0),
                                    ],
                                    egui::Stroke::new(1.5, egui::Color32::from_rgb(255, 88, 88)),
                                );

                                painter.rect_filled(
                                    preview_rect.expand(2.0),
                                    4.0,
                                    egui::Color32::from_rgba_premultiplied(0, 0, 0, 220),
                                );
                                painter.rect_filled(image_rect, 3.0, egui::Color32::from_gray(20));
                                painter.rect_filled(
                                    action_rect,
                                    0.0,
                                    egui::Color32::from_rgba_unmultiplied(0, 0, 0, 235),
                                );
                                painter.rect_stroke(
                                    preview_rect,
                                    3.0,
                                    egui::Stroke::new(1.0, egui::Color32::from_gray(150)),
                                    egui::StrokeKind::Inside,
                                );
                                let thumbnail_matches =
                                    hover_thumbnail.as_ref().is_some_and(|thumb| {
                                        crate::video::thumbnail::is_within_tolerance(
                                            target,
                                            thumb.target_secs,
                                            thumb.match_tolerance_secs,
                                        )
                                    });
                                // スクラブ中はサムネ画像を消さずに直近の 1 枚を出し
                                // 続ける。以前は target に合うサムネが無い間「黒地 +
                                // loading」を出していたため、スクラブ中にサムネ画像と
                                // 黒地が交互に出てちらついていた。
                                if let (Some(texture_id), Some(thumb)) =
                                    (hover_texture_id, hover_thumbnail.as_ref())
                                {
                                    let fitted = fit_rect_in_rect(
                                        egui::vec2(thumb.width as f32, thumb.height as f32),
                                        image_rect,
                                    );
                                    painter.image(
                                        texture_id,
                                        fitted,
                                        egui::Rect::from_min_max(
                                            egui::pos2(0.0, 0.0),
                                            egui::pos2(1.0, 1.0),
                                        ),
                                        egui::Color32::WHITE,
                                    );
                                }
                                // 目標位置のサムネがまだ揃っていない間は、フルスクリーン
                                // の「シーク中...」表示と同じ見た目の box を中央に重ねる
                                // (サムネ未取得時は image_rect の gray 地の上に出る)。
                                if !thumbnail_matches {
                                    let font = egui::FontId::proportional(14.0);
                                    let galley = painter.layout_no_wrap(
                                        "シーク中".to_owned(),
                                        font,
                                        egui::Color32::from_rgb(238, 238, 238),
                                    );
                                    let box_rect = egui::Rect::from_center_size(
                                        image_rect.center(),
                                        galley.size() + egui::vec2(28.0, 16.0),
                                    );
                                    painter.rect_filled(
                                        box_rect,
                                        6.0,
                                        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 214),
                                    );
                                    painter.galley(
                                        box_rect.center() - galley.size() * 0.5,
                                        galley,
                                        egui::Color32::PLACEHOLDER,
                                    );
                                }

                                let action_size = 24.0;
                                let action_gap = 6.0;
                                let pin_rect = egui::Rect::from_min_size(
                                    action_rect.min + egui::vec2(6.0, 4.0),
                                    egui::vec2(action_size, action_size),
                                );
                                let bookmark_rect = egui::Rect::from_min_size(
                                    egui::pos2(pin_rect.max.x + action_gap, pin_rect.min.y),
                                    egui::vec2(action_size, action_size),
                                );
                                let pin_resp = ui.interact(
                                    pin_rect,
                                    egui::Id::new("native_video_hover_pin"),
                                    egui::Sense::click(),
                                );
                                draw_overlay_button_bg(
                                    painter,
                                    pin_rect,
                                    pin_resp.hovered(),
                                    hover_preview_pinned,
                                );
                                draw_overlay_pin_icon(
                                    painter,
                                    pin_rect.center(),
                                    action_size * 0.34,
                                    if hover_preview_pinned {
                                        egui::Color32::from_rgb(180, 255, 180)
                                    } else {
                                        egui::Color32::from_rgb(118, 214, 255)
                                    },
                                );
                                let pin_resp = pin_resp.hover_tip_dark(
                                    native_label_with_shortcut(
                                        if hover_preview_pinned {
                                            "この位置でピン留めを上書き"
                                        } else {
                                            "この位置をピン留め"
                                        },
                                        shortcut_labels.and_then(|s| s.pin.as_deref()),
                                    ),
                                );
                                if pin_resp.clicked() {
                                    commands.push(NativeOverlayCommand::SetPinAt {
                                        target_secs: target,
                                    });
                                }
                                let bookmark_resp = ui.interact(
                                    bookmark_rect,
                                    egui::Id::new("native_video_hover_bookmark"),
                                    egui::Sense::click(),
                                );
                                draw_overlay_button_bg(
                                    painter,
                                    bookmark_rect,
                                    bookmark_resp.hovered(),
                                    hover_preview_bookmarked,
                                );
                                draw_overlay_bookmark_icon(
                                    painter,
                                    bookmark_rect.center(),
                                    action_size * 0.32,
                                    if hover_preview_bookmarked {
                                        egui::Color32::from_rgb(255, 245, 145)
                                    } else {
                                        egui::Color32::from_rgb(255, 220, 80)
                                    },
                                );
                                let bookmark_resp = bookmark_resp.hover_tip_dark(
                                    native_label_with_shortcut(
                                        if hover_preview_bookmarked {
                                            "ブックマーク済み"
                                        } else {
                                            "ブックマークを追加"
                                        },
                                        shortcut_labels.and_then(|s| s.bookmark.as_deref()),
                                    ),
                                );
                                if bookmark_resp.clicked() {
                                    commands.push(NativeOverlayCommand::AddBookmarkAt {
                                        target_secs: target,
                                    });
                                }

                                let time_label = format_overlay_time(target);
                                painter.text(
                                    egui::pos2(action_rect.max.x - 8.0, action_rect.center().y),
                                    egui::Align2::RIGHT_CENTER,
                                    time_label,
                                    crate::ui_fonts::hud_text_font(13.0),
                                    egui::Color32::from_rgb(245, 245, 245),
                                );
                            }
                        }

                        // Inc 5c-B2: speed ボタン + プリセット popup は動画/音楽共有の
                        // `draw_overlay_speed_control` (`overlay_draw`) へ抽出。popup 位置は
                        // overlay 座標 (left=0, width=overlay 幅, top=hud_rect.min.y) を渡す。
                        // popup rect は native HWND の SetWindowRgn 用に
                        // `last_drawn_speed_popup_rect` へ受け取る。
                        if let Some(speed) = draw_overlay_speed_control(
                            ctx,
                            ui,
                            painter,
                            speed_rect,
                            text_center_y,
                            playback_speed,
                            egui::Id::new("native_video_speed"),
                            egui::Id::new("native_video_speed_popup"),
                            0.0,
                            overlay_width_points,
                            hud_rect.min.y,
                            &mut video_speed_popup_open,
                            &mut last_drawn_speed_popup_rect,
                        ) {
                            commands.push(NativeOverlayCommand::SetPlaybackSpeed { speed });
                        }

                        let mute_resp = ui.interact(
                            mute_rect,
                            egui::Id::new("native_video_mute"),
                            egui::Sense::click(),
                        );
                        draw_overlay_button_bg(painter, mute_rect, mute_resp.hovered(), muted);
                        draw_overlay_speaker_icon(
                            painter,
                            mute_rect.center(),
                            btn_size * 0.46,
                            muted,
                        );
                        let mute_resp = mute_resp.hover_tip_dark(native_label_with_shortcut(
                            if muted { "ミュート解除" } else { "ミュート" },
                            shortcut_labels.and_then(|s| s.mute.as_deref()),
                        ));
                        if mute_resp.clicked() {
                            commands.push(NativeOverlayCommand::ToggleMute);
                        }

                        // ── 音量ノーマライズボタン (mute と vol_slider の間) ──
                        use crate::video::normalize_types::NormalizeUiState;
                        let norm_ui_state = self.normalize_state.ui_state;
                        let is_scanning = matches!(norm_ui_state, NormalizeUiState::Scanning);
                        let norm_active = matches!(
                            norm_ui_state,
                            NormalizeUiState::OnApplied { .. }
                                | NormalizeUiState::ProvisionalApplied { .. }
                        );
                        let norm_unmeasured =
                            matches!(norm_ui_state, NormalizeUiState::OnUnmeasured);
                        let norm_resp = ui.interact(
                            norm_rect,
                            egui::Id::new("native_video_normalize"),
                            egui::Sense::CLICK | egui::Sense::HOVER,
                        );
                        let norm_hover_label = match norm_ui_state {
                            NormalizeUiState::Off => {
                                "音量ノーマライズ (-14 LUFS)。クリックで ON".to_string()
                            }
                            NormalizeUiState::OnApplied { gain_db } => format!(
                                "音量ノーマライズ ON ({gain_db:+.1}dB / -14 LUFS)。クリックで OFF"
                            ),
                            NormalizeUiState::ProvisionalApplied { gain_db } => format!(
                                "音量ノーマライズ ON (仮 {gain_db:+.1}dB / 確定測定中)。クリックで OFF"
                            ),
                            NormalizeUiState::OnUnmeasured => {
                                "音量ノーマライズが有効です。クリックして測定 / 右クリックで OFF"
                                    .to_string()
                            }
                            NormalizeUiState::Scanning => "ノーマライズ中…".to_string(),
                        };
                        draw_overlay_button_bg(
                            painter,
                            norm_rect,
                            norm_resp.hovered() && !is_scanning,
                            norm_active,
                        );
                        // ボタン色: Off=グレー / OnApplied=黄 / OnUnmeasured=オレンジ点滅 / Scanning=グレー
                        let norm_color = if is_scanning {
                            egui::Color32::from_gray(120)
                        } else if norm_active {
                            egui::Color32::from_rgb(255, 198, 62)
                        } else if norm_unmeasured {
                            // 半透明 blink (時間ベースで alpha 変動)
                            let t = ui.ctx().input(|i| i.time);
                            let blink = (((t * 2.0).sin() + 1.0) * 0.5) as f32;
                            let alpha = (180.0 + blink * 75.0) as u8;
                            egui::Color32::from_rgba_unmultiplied(255, 150, 60, alpha)
                        } else {
                            egui::Color32::from_gray(180)
                        };
                        // "Norm" ラベル
                        painter.text(
                            egui::pos2(norm_rect.center().x, text_center_y),
                            egui::Align2::CENTER_CENTER,
                            "Norm",
                            crate::ui_fonts::hud_text_font(11.0),
                            norm_color,
                        );
                        let norm_resp = norm_resp.hover_tip_dark(norm_hover_label);
                        if !is_scanning {
                            if norm_resp.clicked() {
                                commands.push(NativeOverlayCommand::ToggleNormalize);
                            } else if norm_resp.secondary_clicked() {
                                commands.push(NativeOverlayCommand::DisableNormalize);
                            }
                        }

                        // Inc 5c-B1: 音量 dB フェーダーは動画/音楽共有の
                        // `draw_overlay_volume_slider` (`overlay_draw`) へ抽出。
                        // `volume` を finite 化した値でシャドウし、後段の音量ラベル
                        // (`format_video_volume_db_compact`) にも同じ値を渡す。
                        let volume = finite_video_volume(volume);
                        let volume_shortcuts = native_joined_shortcuts(&[
                            shortcut_labels.and_then(|s| s.volume_up.as_deref()),
                            shortcut_labels.and_then(|s| s.volume_down.as_deref()),
                        ]);
                        let vol_tooltip = native_label_with_shortcut(
                            "音量 (ダブルクリックで 0dB)",
                            volume_shortcuts.as_deref(),
                        );
                        if let Some((value, persist)) = draw_overlay_volume_slider(
                            ui,
                            painter,
                            vol_rect,
                            volume,
                            egui::Id::new("native_video_volume"),
                            Some(vol_tooltip),
                            &mut self.last_volume_target,
                        ) {
                            commands.push(NativeOverlayCommand::SetVolume {
                                volume: value,
                                persist,
                            });
                        }
                        // 動画 HUD 2 段化リデザイン (実機フィードバック反映): 最小窓
                        // (`CompactionTier::Minimal`) では vol_label と limiter インジケータを
                        // 非表示にして右クラスター幅を縮める。
                        if show_vol_label {
                            let volume_label = format_video_volume_db_compact(volume);
                            let volume_label_color = if volume > 1.0 {
                                egui::Color32::from_rgb(255, 210, 80)
                            } else {
                                egui::Color32::from_rgb(238, 238, 238)
                            };
                            painter.text(
                                egui::pos2(vol_rect.max.x + gap + vol_label_w, text_center_y),
                                egui::Align2::RIGHT_CENTER,
                                volume_label,
                                crate::ui_fonts::hud_text_font(13.0),
                                volume_label_color,
                            );
                        }
                        if show_limiter_slot && limiter_ceiling_hit {
                            let limiter_rect = egui::Rect::from_center_size(
                                egui::pos2(
                                    vol_rect.max.x + gap + vol_label_w + limiter_indicator_w * 0.5,
                                    center_y,
                                ),
                                egui::vec2(limiter_indicator_w, btn_size),
                            );
                            let limiter_resp = ui.interact(
                                limiter_rect,
                                egui::Id::new("native_video_limiter_indicator"),
                                egui::Sense::hover(),
                            );
                            painter.circle_filled(
                                limiter_rect.center(),
                                if limiter_resp.hovered() { 4.5 } else { 4.0 },
                                egui::Color32::from_rgb(255, 72, 72),
                            );
                            limiter_resp.hover_tip_dark("出力リミッターが作動しました");
                        }
                        if !audio_only {
                            let seek_strip_state = seek_strip
                                .as_ref()
                                .map(|strip| {
                                    crate::settings::VideoSeekStripState::from_mode(
                                        strip.center.mode(),
                                    )
                                })
                                .unwrap_or(crate::settings::VideoSeekStripState::None);
                            draw_native_seek_strip_cycle_button(
                                ui,
                                painter,
                                seek_strip_selector_rect,
                                seek_strip_state,
                                seek_strip_unavailable_tooltip,
                                &mut commands,
                            );
                        }
                        draw_native_bar_lock_button(
                            ui,
                            painter,
                            seek_lock_rect,
                            "native_seek_bar_lock",
                            bottom_lock.bar_locked(),
                            if bottom_lock.bar_locked() {
                                "シークバー固定を解除"
                            } else {
                                "シークバーを固定表示"
                            },
                            crate::video::NativeVideoBar::Seek,
                            &mut commands,
                        );

                        if cfg!(debug_assertions) {
                            let pointer = pointer_pos
                                .map(|p| format!("{:.0},{:.0}", p.x, p.y))
                                .unwrap_or_else(|| "outside".to_string());
                            painter.text(
                                egui::pos2(hud_rect.max.x - 12.0, hud_rect.min.y + 10.0),
                                egui::Align2::RIGHT_TOP,
                                format!("native overlay events={event_count} pointer={pointer}"),
                                egui::FontId::proportional(10.0),
                                egui::Color32::from_rgb(150, 150, 150),
                            );
                        }
                        if hud_dimmed {
                            painter.rect_filled(
                                hud_rect,
                                0.0,
                                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 86),
                            );
                        }
                    });
            } // ← `if bottom_hud_visible {` の閉じ (Codex 4周目 P1)
            // Codex 3周目 P2 反映: 音量ノーマライズ進捗パネルは **すべての overlay UI
            // 描画の最後** に置く。同じ Order::Foreground の Area は描画順 = z-order なので、
            // metadata panel / jump panel / bookmark editor / bottom HUD より後に描けば
            // 全画面 blocker がそれらより前面に出てクリックを完全にキャプチャできる。
            // Codex 4周目 P1: bottom_hud_visible == false でも必ず実行 (HUD フェードアウト中も
            // 進捗 UI は出続ける)。
            if matches!(
                normalize_state_snap.ui_state,
                crate::video::normalize_types::NormalizeUiState::Scanning
            ) {
                if let Some(progress) = normalize_state_snap.progress.as_ref() {
                    draw_native_normalize_progress(
                        ctx,
                        overlay_width_points,
                        overlay_height_points,
                        progress,
                        &mut commands,
                    );
                }
            }
            if touch_first_run_help_visible {
                draw_native_video_touch_first_run_help(
                    ctx,
                    overlay_width_points,
                    overlay_height_points,
                    ui_scale,
                );
            }
        });
        let egui_run_ms = egui_run_t0.elapsed().as_secs_f64() * 1000.0;
        let egui_hover_pos = self.egui_ctx.pointer_hover_pos();
        crate::video::cursor_debug::log(format_args!(
            "layer=egui event=presenter_frame epoch={} strip_area_ran={seek_strip_area_ran} strip_present={} bottom_hud_visible={bottom_hud_visible} tile_overlay_visible={tile_overlay_visible} navigation_preview_visible={navigation_preview_visible} native_hover_points={visibility_hover_pos:?} egui_hover_points={egui_hover_pos:?} platform_cursor={:?}",
            self.window_epoch,
            seek_strip.is_some(),
            full_output.platform_output.cursor_icon,
        ));
        // presenter は本体とは別の egui Context を持つので、生成側もここで記録しないと
        // 「produced と applied が対になる」という台帳の読み方が presenter 分だけ崩れる
        // (Codex 2 周目指摘)。詳細: `egui_wgpu::atlas_diag`。
        egui_wgpu::atlas_diag::record_produced(
            egui_wgpu::atlas_diag::Site::ProducedPresenter,
            &full_output.textures_delta,
            None,
            {
                let tex_manager = self.egui_ctx.tex_manager();
                let guard = tex_manager.read();
                guard.meta(egui::TextureId::default()).map(|meta| meta.size)
            },
        );
        let repaint_delay = full_output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|output| output.repaint_delay)
            .unwrap_or(Duration::MAX);
        self.next_repaint_deadline = if repaint_delay == Duration::MAX {
            None
        } else {
            Instant::now().checked_add(repaint_delay)
        };
        let now = Instant::now();
        self.schedule_seek_status_repaint(now);
        self.schedule_limiter_indicator_repaint(now);
        if hud_repaint_debug_enabled() && repaint_delay != Duration::MAX {
            crate::logger::log(format!(
                "[HUD-DEBUG] egui repaint_delay={:?} pending={} dirty_after_run={}",
                repaint_delay,
                self.pending_events.len(),
                self.dirty
            ));
        }
        // Query after `run`: egui updates these flags from the just-processed
        // frame, which lets this presenter decide whether to forward the same
        // native input batch to the legacy fullscreen shortcut path.
        self.wants_pointer_input = self.egui_ctx.wants_pointer_input();
        self.wants_keyboard_input = self.egui_ctx.wants_keyboard_input();
        self.last_seek_target_secs = last_seek_target_secs;
        // T35: hover_preview_target_secs が Some → None に遷移したら clear イベントを
        // 送って player 側の pump_native_hover_thumbnail 永久リトライを止める
        let had_hover = self.hover_preview_target_secs.is_some();
        let has_hover = hover_preview_target_secs.is_some();
        if had_hover && !has_hover {
            commands.push(NativeOverlayCommand::ClearSeekThumbnail);
            // A later hover at the same position must issue a fresh request after the worker-side
            // clear. Keeping the old debounce key would otherwise suppress that first request.
            last_thumbnail_request_secs = None;
            last_thumbnail_request_at = None;
        }
        self.last_thumbnail_request_secs = last_thumbnail_request_secs;
        self.last_thumbnail_request_at = last_thumbnail_request_at;
        self.hover_preview_target_secs = hover_preview_target_secs;
        self.hover_preview_anchor_x = hover_preview_anchor_x;
        self.last_drawn_preview_rect = last_drawn_preview_rect;
        self.last_drawn_vst3_panel_rect = last_drawn_vst3_panel_rect;
        self.last_emitted_vst3_panel_pos = last_emitted_vst3_panel_pos;
        self.last_drawn_toast_rect = last_drawn_toast_rect;
        self.last_drawn_speed_popup_rect = last_drawn_speed_popup_rect;
        self.last_drawn_bookmark_editor_rect = last_drawn_bookmark_editor_rect;
        self.last_drawn_bulk_bookmark_dialog_rect = last_drawn_bulk_bookmark_dialog_rect;
        self.last_drawn_shortcut_help_rect = last_drawn_shortcut_help_rect;
        self.last_drawn_ring_picker_rect = last_drawn_ring_picker_rect;
        self.last_drawn_ring_guide_rect = last_drawn_ring_guide_rect;
        self.video_speed_popup_open = video_speed_popup_open;
        self.frame_step_hold = frame_step_hold;
        self.seek_row_gesture = seek_row_gesture;
        self.seek_strip_drag_origin = seek_strip_drag_origin;
        self.last_seek_strip_window_request = last_seek_strip_window_request;
        self.bookmark_title_edit = bookmark_title_edit;
        self.bulk_bookmark_dialog = bulk_bookmark_dialog;
        self.video_left_panel_tab = video_left_panel_tab;
        self.video_adjustment_tab = video_adjustment_tab;
        self.video_adjustments = video_adjustments;
        self.video_scale_filter = video_scale_filter;
        self.video_downscale_smoothing_percent = video_downscale_smoothing_percent;
        self.video_anime4k_budget = video_anime4k_budget;
        self.shortcut_help_open = shortcut_help_open;
        if let Some(intent) = self.maybe_claim_text_input_focus() {
            window_intents.push(intent);
        }
        self.top_bar_drawn_visible = bar_visibility.top_bar_visible;
        self.bottom_hud_visible = bar_visibility.bottom_hud_visible;
        self.top_bar_visible = top_bar_visible || side_panel_visible;
        self.right_panel_visible = right_panel_visible;
        self.jump_panel_visible = jump_panel_visible;
        self.left_panel_open = left_panel_open;
        let left_panel_open_changed = left_panel_open != left_panel_open_before;
        // egui 側で発行されたクリップボードコピー (Ctrl+C / Ctrl+X 応答) を OS に流す。
        for cmd in &full_output.platform_output.commands {
            if let egui::OutputCommand::CopyText(text) = cmd {
                write_clipboard_text_windows(text);
            }
        }
        if let Some(intent) = self.ime_cursor_area_intent(full_output.platform_output.ime) {
            window_intents.push(intent);
        }
        window_intents.push(NativeWindowIntent::SetCursorPolicy {
            icon: Self::resolve_cursor_icon(full_output.platform_output.cursor_icon),
            auto_hide_allowed: !cursor_blocking_overlay_visible,
        });

        let shape_count = full_output.shapes.len();
        // Pure CPU tessellation is deliberately outside the epoch gate.
        let tessellate_t0 = Instant::now();
        let paint_jobs = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);
        let tessellate_ms = tessellate_t0.elapsed().as_secs_f64() * 1000.0;
        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.width, self.height],
            pixels_per_point: full_output.pixels_per_point,
        };

        // Two native-video-render threads can briefly share one DeviceEpoch. NOT because
        // two videos play at once -- only one live media session exists at a time
        // (`close_parked_live_media_windows_for_new_media`), and an F12 placement switch
        // builds and drops both presenters on this same thread. The overlap comes from
        // teardown: `Drop for NativeVideoOutput` sets cancel and hands the joins to a
        // separate thread instead of blocking, so the outgoing render thread can still
        // finish a submit while the incoming one configures its surface. Before the device
        // was shared each overlay had its own, so this could not happen.
        //
        // A read guard covers all texture/queue/surface GPU work through present; configure
        // takes the matching write guard. Context::run and tessellation stay outside.
        // `gate_wait_ms` below reports whether the overlap is ever actually observed.
        let gpu_span_t0 = Instant::now();
        let gate_wait_t0 = Instant::now();
        let _submission_guard = self.gpu_epoch.submission_guard();
        let gate_wait_ms = gate_wait_t0.elapsed().as_secs_f64() * 1000.0;
        self.gpu_epoch.ensure_alive("overlay GPU submission")?;

        // presenter は本体とは別の egui Context / Renderer を持つ。font atlas の追跡台帳は
        // renderer 番号で区別するので、こちらの適用も同じ経路に載せる
        // (詳細: `egui_wgpu::atlas_diag`)。
        let diag_id = self.renderer.diag_id();
        let atlas_batch = egui_wgpu::atlas_diag::begin_applied_batch(
            egui_wgpu::atlas_diag::Site::AppliedPresenter,
            diag_id,
            &full_output.textures_delta,
        );
        let texture_update_t0 = Instant::now();
        for (id, image_delta) in &full_output.textures_delta.set {
            let before = self.renderer.texture_size(id);
            self.renderer.update_texture(
                self.gpu_epoch.device(),
                self.gpu_epoch.queue(),
                *id,
                image_delta,
            );
            let after = self.renderer.texture_size(id);
            egui_wgpu::atlas_diag::record_applied(*id, diag_id, image_delta, before, after);
        }
        let texture_update_ms = texture_update_t0.elapsed().as_secs_f64() * 1000.0;
        if atlas_batch.tracked() {
            egui_wgpu::atlas_diag::flush("applying the presenter's texture delta batch");
        }

        let surface_acquire_t0 = Instant::now();
        let surface_texture = match self.surface.get_current_texture() {
            Ok(texture) => texture,
            Err(error) => {
                if error == wgpu::SurfaceError::Other && self.gpu_epoch.is_lost() {
                    return Err(format!(
                        "wgpu overlay device epoch {} was lost during surface acquire: {error:?}",
                        self.gpu_epoch.generation()
                    ));
                }
                // Lost / Outdated / Timeout are surface conditions. They preserve the
                // epoch cache; only the device-lost callback can latch an epoch.
                return Err(format!("egui overlay get_current_texture: {error:?}"));
            }
        };
        let surface_acquire_ms = surface_acquire_t0.elapsed().as_secs_f64() * 1000.0;
        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let buffer_update_encode_t0 = Instant::now();
        let mut encoder =
            self.gpu_epoch
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("mIV native egui overlay encoder"),
                });
        let user_cmds = self.renderer.update_buffers(
            self.gpu_epoch.device(),
            self.gpu_epoch.queue(),
            &mut encoder,
            &paint_jobs,
            &screen_descriptor,
        );
        {
            let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mIV native egui overlay render"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            self.renderer.render(
                &mut render_pass.forget_lifetime(),
                &paint_jobs,
                &screen_descriptor,
            );
        }
        let mut submissions = user_cmds;
        submissions.push(encoder.finish());
        let buffer_update_encode_ms = buffer_update_encode_t0.elapsed().as_secs_f64() * 1000.0;
        let submit_present_t0 = Instant::now();
        self.gpu_epoch.queue().submit(submissions);
        {
            let _operation = self
                .health
                .begin_render_operation(NativeRenderOperation::Present, self.window_epoch);
            surface_texture.present();
        }
        let submit_present_ms = submit_present_t0.elapsed().as_secs_f64() * 1000.0;
        let gpu_span_ms = gpu_span_t0.elapsed().as_secs_f64() * 1000.0;
        drop(_submission_guard);
        if overlay_visible {
            self.set_visual_attached(true)?;
        }
        for id in &full_output.textures_delta.free {
            self.renderer.free_texture(id);
        }
        self.dirty = self.frame_step_hold.is_some()
            || self.bookmark_title_edit.is_some()
            || self.bulk_bookmark_dialog.is_some()
            || self.shortcut_help_open
            || left_panel_open_changed;
        self.render_count = self.render_count.saturating_add(1);
        log_event(
            "egui_overlay_present",
            &[
                ("width", Value::from(self.width as i64)),
                ("height", Value::from(self.height as i64)),
                ("input_events", Value::from(pending_event_count as i64)),
                ("native_events", Value::from(event_count as i64)),
                ("pixels_per_point", Value::from(ppp)),
                ("first_render", Value::from(first_render)),
                (
                    "device_generation",
                    Value::from(self.gpu_epoch.generation()),
                ),
                ("shapes", Value::from(shape_count as i64)),
                ("paint_jobs", Value::from(paint_jobs.len() as i64)),
                (
                    "texture_set_count",
                    Value::from(full_output.textures_delta.set.len() as i64),
                ),
                ("wants_pointer", Value::from(self.wants_pointer_input)),
                ("wants_keyboard", Value::from(self.wants_keyboard_input)),
                ("hud_visible", Value::from(hud_visible)),
                ("perf_visible", Value::from(perf_visible)),
                ("visual_attached", Value::from(self.visual_attached)),
                ("egui_run_ms", Value::from(egui_run_ms)),
                ("tessellate_ms", Value::from(tessellate_ms)),
                ("gate_wait_ms", Value::from(gate_wait_ms)),
                ("texture_update_ms", Value::from(texture_update_ms)),
                ("surface_acquire_ms", Value::from(surface_acquire_ms)),
                (
                    "buffer_update_encode_ms",
                    Value::from(buffer_update_encode_ms),
                ),
                ("submit_present_ms", Value::from(submit_present_ms)),
                ("gpu_span_ms", Value::from(gpu_span_ms)),
                (
                    "render_ms",
                    Value::from(render_t0.elapsed().as_secs_f64() * 1000.0),
                ),
            ],
        );
        Ok((commands, window_intents))
    }

    fn ime_cursor_area_intent(
        &self,
        ime: Option<egui::output::IMEOutput>,
    ) -> Option<NativeWindowIntent> {
        let ime = ime?;
        let ppp = self.pixels_per_point;
        let cursor = ime.cursor_rect;
        let x = (cursor.min.x * ppp).round() as i32;
        let y = (cursor.min.y * ppp).round() as i32;
        let width = (cursor.width().max(1.0) * ppp).round() as i32;
        let height = (cursor.height().max(1.0) * ppp).round() as i32;
        Some(NativeWindowIntent::UpdateImeCursorArea {
            x,
            y,
            width,
            height,
        })
    }

    fn resolve_cursor_icon(cursor_icon: egui::CursorIcon) -> NativeCursorIcon {
        match cursor_icon {
            egui::CursorIcon::None => NativeCursorIcon::Hidden,
            egui::CursorIcon::PointingHand => NativeCursorIcon::Hand,
            egui::CursorIcon::Text | egui::CursorIcon::VerticalText => NativeCursorIcon::Text,
            egui::CursorIcon::ResizeHorizontal => NativeCursorIcon::ResizeHorizontal,
            egui::CursorIcon::ResizeVertical => NativeCursorIcon::ResizeVertical,
            egui::CursorIcon::Move
            | egui::CursorIcon::Grab
            | egui::CursorIcon::Grabbing
            | egui::CursorIcon::AllScroll => NativeCursorIcon::Move,
            egui::CursorIcon::NotAllowed | egui::CursorIcon::NoDrop => NativeCursorIcon::NotAllowed,
            egui::CursorIcon::Progress | egui::CursorIcon::Wait => NativeCursorIcon::Wait,
            egui::CursorIcon::Default => NativeCursorIcon::Arrow,
            _ => NativeCursorIcon::Arrow,
        }
    }
}

fn choose_overlay_surface_format(
    formats: &[wgpu::TextureFormat],
) -> Result<wgpu::TextureFormat, String> {
    for preferred in [
        wgpu::TextureFormat::Bgra8Unorm,
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Bgra8UnormSrgb,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ] {
        if formats.contains(&preferred) {
            return Ok(preferred);
        }
    }
    formats
        .first()
        .copied()
        .ok_or_else(|| "wgpu DComp overlay surface has no formats".to_string())
}

pub(crate) fn effective_overlay_pixels_per_point(os_ppp: f32, ui_scale: f32) -> f32 {
    let os_ppp = if os_ppp.is_finite() && os_ppp > 0.0 {
        os_ppp
    } else {
        1.0
    };
    os_ppp * crate::settings::normalize_ui_scale_factor(ui_scale)
}

fn egui_modifiers(shift: bool, ctrl: bool, alt: bool) -> egui::Modifiers {
    egui::Modifiers {
        alt,
        ctrl,
        shift,
        mac_cmd: false,
        command: ctrl,
    }
}

fn egui_key_from_virtual_key(vk: u32) -> Option<egui::Key> {
    Some(match vk {
        0x08 => egui::Key::Backspace,
        0x09 => egui::Key::Tab,
        0x0D => egui::Key::Enter,
        0x1B => egui::Key::Escape,
        0x20 => egui::Key::Space,
        0x21 => egui::Key::PageUp,
        0x22 => egui::Key::PageDown,
        0x23 => egui::Key::End,
        0x24 => egui::Key::Home,
        0x25 => egui::Key::ArrowLeft,
        0x26 => egui::Key::ArrowUp,
        0x27 => egui::Key::ArrowRight,
        0x28 => egui::Key::ArrowDown,
        0x2D => egui::Key::Insert,
        0x2E => egui::Key::Delete,
        0x30 => egui::Key::Num0,
        0x31 => egui::Key::Num1,
        0x32 => egui::Key::Num2,
        0x33 => egui::Key::Num3,
        0x34 => egui::Key::Num4,
        0x35 => egui::Key::Num5,
        0x36 => egui::Key::Num6,
        0x37 => egui::Key::Num7,
        0x38 => egui::Key::Num8,
        0x39 => egui::Key::Num9,
        0x41 => egui::Key::A,
        0x42 => egui::Key::B,
        0x43 => egui::Key::C,
        0x44 => egui::Key::D,
        0x45 => egui::Key::E,
        0x46 => egui::Key::F,
        0x47 => egui::Key::G,
        0x48 => egui::Key::H,
        0x49 => egui::Key::I,
        0x4A => egui::Key::J,
        0x4B => egui::Key::K,
        0x4C => egui::Key::L,
        0x4D => egui::Key::M,
        0x4E => egui::Key::N,
        0x4F => egui::Key::O,
        0x50 => egui::Key::P,
        0x51 => egui::Key::Q,
        0x52 => egui::Key::R,
        0x53 => egui::Key::S,
        0x54 => egui::Key::T,
        0x55 => egui::Key::U,
        0x56 => egui::Key::V,
        0x57 => egui::Key::W,
        0x58 => egui::Key::X,
        0x59 => egui::Key::Y,
        0x5A => egui::Key::Z,
        0x70 => egui::Key::F1,
        0x71 => egui::Key::F2,
        0x72 => egui::Key::F3,
        0x73 => egui::Key::F4,
        0x74 => egui::Key::F5,
        0x75 => egui::Key::F6,
        0x76 => egui::Key::F7,
        0x77 => egui::Key::F8,
        0x78 => egui::Key::F9,
        0x79 => egui::Key::F10,
        0x7A => egui::Key::F11,
        0x7B => egui::Key::F12,
        0x7C => egui::Key::F13,
        0x7D => egui::Key::F14,
        0x7E => egui::Key::F15,
        0x7F => egui::Key::F16,
        0x80 => egui::Key::F17,
        0x81 => egui::Key::F18,
        0x82 => egui::Key::F19,
        0x83 => egui::Key::F20,
        0x84 => egui::Key::F21,
        0x85 => egui::Key::F22,
        0x86 => egui::Key::F23,
        0x87 => egui::Key::F24,
        _ => return None,
    })
}

impl NativeTestOverlay {
    fn new(
        factory: &IDXGIFactory2,
        d3d_device: &ID3D11Device,
        d3d_device1: &ID3D11Device1,
        d3d_context: &ID3D11DeviceContext,
        d3d_context1: &ID3D11DeviceContext1,
        dcomp_device: &IDCompositionDevice,
        width: u32,
        height: u32,
        transparent: bool,
    ) -> Result<Self, String> {
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width.max(1),
            Height: height.max(1),
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            Flags: 0,
        };
        let swap_chain = unsafe {
            factory
                .CreateSwapChainForComposition(d3d_device, &desc, None::<&IDXGIOutput>)
                .map_err(|e| format!("CreateSwapChainForComposition overlay: {e:?}"))?
        };
        let visual = unsafe {
            let visual = dcomp_device
                .CreateVisual()
                .map_err(|e| format!("CreateVisual overlay: {e:?}"))?;
            visual
                .SetContent(&swap_chain)
                .map_err(|e| format!("IDCompositionVisual::SetContent overlay: {e:?}"))?;
            visual
        };
        let mut this = Self {
            swap_chain,
            _visual: visual,
            backbuffer: None,
            render_target: None,
            width: width.max(1),
            height: height.max(1),
            transparent,
        };
        this.recreate_backbuffer(d3d_device1, d3d_context)?;
        if transparent {
            this.present_transparent()?;
        } else {
            this.draw_test_pattern(d3d_context, d3d_context1)?;
        }
        log_event(
            "overlay_init",
            &[
                ("width", Value::from(this.width as i64)),
                ("height", Value::from(this.height as i64)),
                ("alpha_mode", Value::from("premultiplied")),
                ("transparent", Value::from(transparent)),
            ],
        );
        Ok(this)
    }

    fn resize(
        &mut self,
        d3d_device1: &ID3D11Device1,
        d3d_context: &ID3D11DeviceContext,
        d3d_context1: &ID3D11DeviceContext1,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let width = width.max(1);
        let height = height.max(1);
        // T26 (Claude R3-6): backbuffer None なら early-return しない。background swap chain と
        // 同じ理由で、前回 `recreate_backbuffer` 失敗時の half-dead 固着を回避する。
        if self.width == width && self.height == height && self.backbuffer.is_some() {
            return Ok(());
        }
        self.render_target = None;
        self.backbuffer = None;
        unsafe {
            self.swap_chain
                .ResizeBuffers(
                    0,
                    width,
                    height,
                    DXGI_FORMAT_UNKNOWN,
                    DXGI_SWAP_CHAIN_FLAG(0),
                )
                .map_err(|e| format!("overlay IDXGISwapChain::ResizeBuffers: {e:?}"))?;
        }
        self.width = width;
        self.height = height;
        self.recreate_backbuffer(d3d_device1, d3d_context)?;
        if self.transparent {
            self.present_transparent()?;
        } else {
            self.draw_test_pattern(d3d_context, d3d_context1)?;
        }
        Ok(())
    }

    fn recreate_backbuffer(
        &mut self,
        d3d_device1: &ID3D11Device1,
        d3d_context: &ID3D11DeviceContext,
    ) -> Result<(), String> {
        let backbuffer: ID3D11Texture2D = unsafe {
            self.swap_chain
                .GetBuffer(0)
                .map_err(|e| format!("overlay IDXGISwapChain::GetBuffer: {e:?}"))?
        };
        let mut render_target = None;
        unsafe {
            d3d_device1
                .CreateRenderTargetView(&backbuffer, None, Some(&mut render_target))
                .map_err(|e| format!("overlay CreateRenderTargetView: {e:?}"))?;
        }
        let render_target: ID3D11RenderTargetView = render_target
            .ok_or_else(|| "overlay CreateRenderTargetView returned null".to_string())?;
        unsafe {
            d3d_context.ClearRenderTargetView(&render_target, &[0.0, 0.0, 0.0, 0.0]);
        }
        self.backbuffer = Some(backbuffer);
        self.render_target = Some(render_target);
        Ok(())
    }

    fn draw_test_pattern(
        &mut self,
        d3d_context: &ID3D11DeviceContext,
        d3d_context1: &ID3D11DeviceContext1,
    ) -> Result<(), String> {
        let render_target = self
            .render_target
            .as_ref()
            .ok_or_else(|| "overlay render target is not initialized".to_string())?;
        unsafe {
            d3d_context.ClearRenderTargetView(render_target, &[0.0, 0.0, 0.0, 0.0]);
            let view: ID3D11View = render_target
                .cast()
                .map_err(|e| format!("cast overlay RTV to view: {e:?}"))?;
            let bar = RECT {
                left: 32,
                top: 32,
                right: (self.width as i32).min(360).max(33),
                bottom: (self.height as i32).min(92).max(33),
            };
            let badge = RECT {
                left: 44,
                top: 44,
                right: (self.width as i32).min(84).max(45),
                bottom: (self.height as i32).min(80).max(45),
            };
            // Premultiplied colors: RGB components are intentionally <= alpha.
            d3d_context1.ClearView(&view, &[0.0, 0.09, 0.04, 0.34], Some(&[bar]));
            d3d_context1.ClearView(&view, &[0.0, 0.24, 0.10, 0.52], Some(&[badge]));
            self.swap_chain
                .Present(1, Default::default())
                .ok()
                .map_err(|e| format!("overlay IDXGISwapChain::Present: {e:?}"))?;
        }
        log_event(
            "overlay_present",
            &[
                ("width", Value::from(self.width as i64)),
                ("height", Value::from(self.height as i64)),
            ],
        );
        Ok(())
    }

    /// MPO 防止カバー用: 透明クリア済み backbuffer を 1 度だけ present する。
    /// `recreate_backbuffer` が既に `[0,0,0,0]` でクリア済みなので present のみ行う。
    fn present_transparent(&self) -> Result<(), String> {
        unsafe {
            self.swap_chain
                .Present(1, Default::default())
                .ok()
                .map_err(|e| format!("overlay transparent Present: {e:?}"))?;
        }
        Ok(())
    }
}

impl Drop for NativeRenderCore {
    fn drop(&mut self) {
        let _operation = self
            .health
            .begin_render_operation(NativeRenderOperation::Detach, self.window_epoch);
        if !self.waitable.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.waitable);
            }
        }
    }
}

impl NativeRenderCore {
    pub(crate) fn detach(mut self) -> NativeRenderCoreDetachTimings {
        let health = Arc::clone(&self.health);
        let epoch = self.window_epoch;
        let operation = health.begin_render_operation(NativeRenderOperation::Detach, epoch);

        let overlay_t0 = Instant::now();
        drop(self.egui_overlay.take());
        let egui_overlay_ms = overlay_t0.elapsed().as_secs_f64() * 1000.0;

        let rest_t0 = Instant::now();
        drop(self);
        let rest_ms = rest_t0.elapsed().as_secs_f64() * 1000.0;
        drop(operation);
        NativeRenderCoreDetachTimings {
            egui_overlay_ms,
            rest_ms,
        }
    }
}

fn log_event(kind: &str, fields: &[(&str, Value)]) {
    crate::perf::event("native_presenter", kind, None, 0, fields);
}

fn transform_geometry_for_surface(
    content: VideoSurfaceContent,
    sar_num: u32,
    sar_den: u32,
    orientation: VideoOrientation,
) -> (u32, u32, VideoOrientation) {
    match content {
        VideoSurfaceContent::LegacySource => (sar_num, sar_den, orientation),
        // The resample output already materializes SAR and orientation into the
        // physical display rectangle. Reapplying either in DComp would distort
        // or rotate the resolved pixels a second time.
        VideoSurfaceContent::DisplayResolved | VideoSurfaceContent::PanoramaResolved => {
            (1, 1, VideoOrientation::IDENTITY)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct VideoVisualTargetRect {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) width: f32,
    pub(super) height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct VideoVisualLayout {
    pub(super) compact: bool,
    pub(super) pixels_per_point: f32,
    pub(super) top_bar_locked: bool,
    pub(super) bottom_lock: VideoBottomLock,
    /// Presenter に渡されたストリップ snapshot (`Some` = 表示中) の有無。
    pub(super) seek_strip_visible: bool,
    pub(super) fixed_bar_gap_px: u32,
}

impl From<bool> for VideoVisualLayout {
    fn from(compact: bool) -> Self {
        Self {
            compact,
            pixels_per_point: 1.0,
            top_bar_locked: false,
            bottom_lock: VideoBottomLock::None,
            seek_strip_visible: false,
            fixed_bar_gap_px: 0,
        }
    }
}

pub(super) fn compute_video_visual_target_rect(
    win_w: u32,
    win_h: u32,
    layout: VideoVisualLayout,
) -> VideoVisualTargetRect {
    let win_w = win_w.max(1) as f32;
    let win_h = win_h.max(1) as f32;
    let ppp = if layout.pixels_per_point.is_finite() {
        layout.pixels_per_point.max(0.01)
    } else {
        1.0
    };
    let gap_points = layout
        .fixed_bar_gap_px
        .min(crate::settings::FULLSCREEN_FIXED_BAR_GAP_MAX_PX) as f32;
    let top_reserved = if layout.top_bar_locked {
        (HUD_TOP_HEIGHT + gap_points) * ppp
    } else {
        0.0
    };
    let bottom_reserved = if layout.bottom_lock.bar_locked() {
        let strip_height = if layout.bottom_lock.strip_locked() && layout.seek_strip_visible {
            SEEK_STRIP_HEIGHT
        } else {
            0.0
        };
        (HUD_BOTTOM_HEIGHT + gap_points + strip_height) * ppp
    } else {
        0.0
    };
    let content_top = top_reserved.min(win_h - 1.0);
    let content_bottom = (win_h - bottom_reserved).max(content_top + 1.0).min(win_h);
    let content_height = (content_bottom - content_top).max(1.0);

    if layout.compact {
        VideoVisualTargetRect {
            x: win_w * 0.5,
            y: content_top,
            width: win_w * 0.5,
            height: content_height * 0.5,
        }
    } else {
        VideoVisualTargetRect {
            x: 0.0,
            y: content_top,
            width: win_w,
            height: content_height,
        }
    }
}

/// 動画 visual の DirectComposition transform 行列を SAR + display matrix 込みで計算する。
///
/// `surface_w/h` は現在の swap chain backbuffer サイズ。source-resolution surface なら
/// decoded frame の raw pixel サイズ、display-resolution surface なら表示矩形の物理 px。
/// `win_w/h` はウィンドウサイズ。SAR は raw X 軸へ、orientation はその後の表示軸へ
/// 適用する。固定バーの領域を上下から除外した後、`compact` なら残った領域の
/// 右上 1/4 へさらに絞り込む。
///
/// 戻り値は `(M11, M12, M21, M22, M31, M32)`。orientation=0 では旧 SAR 実装と
/// 完全に同じ値を返す。90/270 度では fit 対象の表示幅・高さを転置する。
#[cfg(test)]
fn compute_video_visual_transform(
    surface_w: u32,
    surface_h: u32,
    win_w: u32,
    win_h: u32,
    sar_num: u32,
    sar_den: u32,
    orientation: VideoOrientation,
    layout: impl Into<VideoVisualLayout>,
) -> (f32, f32, f32, f32, f32, f32) {
    compute_video_visual_transform_for_surface(
        surface_w,
        surface_h,
        win_w,
        win_h,
        sar_num,
        sar_den,
        orientation,
        layout,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
fn compute_video_visual_transform_for_surface(
    surface_w: u32,
    surface_h: u32,
    win_w: u32,
    win_h: u32,
    sar_num: u32,
    sar_den: u32,
    orientation: VideoOrientation,
    layout: impl Into<VideoVisualLayout>,
    fill_target: bool,
) -> (f32, f32, f32, f32, f32, f32) {
    let surface_w = surface_w.max(1) as f32;
    let surface_h = surface_h.max(1) as f32;
    let layout = layout.into();
    let sar = (sar_num.max(1) as f32) / (sar_den.max(1) as f32);
    let corrected_w = surface_w * sar;
    let corrected_h = surface_h;
    let (o11, o12, o21, o22) = orientation.matrix_2x2();
    let o11 = f32::from(o11);
    let o12 = f32::from(o12);
    let o21 = f32::from(o21);
    let o22 = f32::from(o22);
    let display_w = o11.abs() * corrected_w + o21.abs() * corrected_h;
    let display_h = o12.abs() * corrected_w + o22.abs() * corrected_h;
    let target = compute_video_visual_target_rect(win_w, win_h, layout);
    let (target_x, target_y, target_w, target_h) =
        (target.x, target.y, target.width, target.height);
    if fill_target {
        return (
            target_w / surface_w,
            0.0,
            0.0,
            target_h / surface_h,
            target_x,
            target_y,
        );
    }
    // `display_w × display_h` を `target_w × target_h` に letterbox fit する scale。
    let scale = (target_w / display_w).min(target_h / display_h);
    // SAR は raw X 軸にだけ掛け、その後に orientation を合成する。
    let m11 = scale * sar * o11;
    let m12 = scale * sar * o12;
    let m21 = scale * o21;
    let m22 = scale * o22;

    // 回転・反転後の bounding box を target 内で中央寄せする。負方向へ伸びる
    // quarter-turn/反転はここで原点へ戻す。
    let x_w = surface_w * m11;
    let x_h = surface_h * m21;
    let y_w = surface_w * m12;
    let y_h = surface_h * m22;
    let min_x = 0.0_f32.min(x_w).min(x_h).min(x_w + x_h);
    let min_y = 0.0_f32.min(y_w).min(y_h).min(y_w + y_h);
    let offset_x = target_x + (target_w - display_w * scale) * 0.5 - min_x;
    let offset_y = target_y + (target_h - display_h * scale) * 0.5 - min_y;
    (m11, m12, m21, m22, offset_x, offset_y)
}

fn copy_cpu_rgba_to_swapchain_bgra(
    src_rgba: &[u8],
    dst_bgra: &mut Vec<u8>,
    width: u32,
    height: u32,
) -> Result<(), String> {
    let expected_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| format!("CPU frame size overflow: {width}x{height}"))?;
    if src_rgba.len() < expected_len {
        return Err(format!(
            "CPU frame buffer is too small: got {} bytes, expected {expected_len}",
            src_rgba.len()
        ));
    }

    dst_bgra.resize(expected_len, 0);
    for (src, dst) in src_rgba[..expected_len]
        .chunks_exact(4)
        .zip(dst_bgra.chunks_exact_mut(4))
    {
        dst[0] = src[2];
        dst[1] = src[1];
        dst[2] = src[0];
        dst[3] = src[3];
    }
    Ok(())
}

fn sample_cpu_rgba_pixel(
    src_rgba: &[u8],
    width: u32,
    height: u32,
    format: i32,
) -> Result<NativePixelSample, String> {
    if width == 0 || height == 0 {
        return Err("CPU pixel probe: empty frame".to_string());
    }
    let x = width / 2;
    let y = height / 2;
    let offset = (y as usize)
        .checked_mul(width as usize)
        .and_then(|row| row.checked_add(x as usize))
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| format!("CPU pixel probe size overflow: {width}x{height}"))?;
    if src_rgba.len() < offset + 4 {
        return Err(format!(
            "CPU pixel probe buffer is too small: got {} bytes, need {}",
            src_rgba.len(),
            offset + 4
        ));
    }
    Ok(NativePixelSample {
        x,
        y,
        width,
        height,
        format,
        b: src_rgba[offset + 2],
        g: src_rgba[offset + 1],
        r: src_rgba[offset],
        a: src_rgba[offset + 3],
    })
}

fn compare_pixel_probe(
    path: &str,
    expected: NativePixelSample,
    actual: NativePixelSample,
) -> Result<(), String> {
    const TOLERANCE: u8 = 0;
    let mismatch = expected.width != actual.width
        || expected.height != actual.height
        || expected.format != actual.format
        || channel_delta(expected.b, actual.b) > TOLERANCE
        || channel_delta(expected.g, actual.g) > TOLERANCE
        || channel_delta(expected.r, actual.r) > TOLERANCE
        || channel_delta(expected.a, actual.a) > TOLERANCE;
    if !mismatch {
        log_event(
            "pixel_probe_match",
            &[
                ("path", Value::from(path)),
                ("x", Value::from(actual.x as i64)),
                ("y", Value::from(actual.y as i64)),
                ("b", Value::from(actual.b as i64)),
                ("g", Value::from(actual.g as i64)),
                ("r", Value::from(actual.r as i64)),
                ("a", Value::from(actual.a as i64)),
            ],
        );
        return Ok(());
    }

    log_event(
        "pixel_probe_mismatch",
        &[
            ("path", Value::from(path)),
            ("expected_b", Value::from(expected.b as i64)),
            ("expected_g", Value::from(expected.g as i64)),
            ("expected_r", Value::from(expected.r as i64)),
            ("expected_a", Value::from(expected.a as i64)),
            ("actual_b", Value::from(actual.b as i64)),
            ("actual_g", Value::from(actual.g as i64)),
            ("actual_r", Value::from(actual.r as i64)),
            ("actual_a", Value::from(actual.a as i64)),
        ],
    );
    Err(format!(
        "native presenter pixel probe mismatch on {path}: expected BGRA=({},{},{},{}) got BGRA=({},{},{},{})",
        expected.b, expected.g, expected.r, expected.a, actual.b, actual.g, actual.r, actual.a
    ))
}

fn channel_delta(a: u8, b: u8) -> u8 {
    a.abs_diff(b)
}

#[cfg(test)]
mod tests {
    use super::{
        HUD_BOTTOM_HEIGHT, HUD_TOP_HEIGHT, NativeBarVisibilitySnapshot, NativeEguiOverlay,
        NativeJumpPanelVisibilityInputs, NativeOverlayInputRouting, NativeOverlaySeekStrip,
        NativePixelSample, NativeRightPanelVisibilityInputs, NativeTouchPanelHandleInputs,
        PreparedVideoScaleSettings, SEEK_STRIP_HEIGHT, VideoOrientation,
        VideoScalePreparationSignature, VideoSurfaceContent, VideoVisualLayout,
        VideoVisualTargetRect, compare_pixel_probe, compute_video_visual_target_rect,
        compute_video_visual_transform, compute_video_visual_transform_for_surface,
        configure_overlay_style, copy_cpu_rgba_to_swapchain_bgra, cursor_move_is_activity,
        draw_native_seek_strip, effective_overlay_pixels_per_point, egui_key_from_virtual_key,
        immediate_native_wheel_command, metadata_clean_text, native_jump_panel_visible_from_inputs,
        native_panel_callout_hud_rects, native_right_panel_visible_from_inputs,
        native_touch_owned_panel_sides, native_touch_panel_handle_hud_rects,
        native_touch_panel_tap_command_dismisses_before_dispatch,
        native_video_fullscreen_shortcut_key, sample_cpu_rgba_pixel, should_claim_text_input_focus,
        validate_prepared_video_scale_settings, video_scale_signature_changed_fields,
    };
    use crate::panorama::{PanoPose, PanoUvTransform};
    use crate::settings::{FsSidePanelMode, VideoBottomLock};
    use crate::video::native_presenter::overlay_draw::{
        NATIVE_TOUCH_PANEL_HANDLE_WIDTH_PT, native_panel_callout_arrow_direction,
        native_panel_callout_bar_rect, native_panel_top, native_seek_bar_lock_button_rect,
        native_seek_strip_selector_button_rect, native_top_bar_lock_button_rect,
    };
    use crate::video::native_window::{
        NativeVideoKeyEvent, NativeVideoMouseButton, NativeVideoMouseButtonEvent,
        NativeVideoMouseEvent, NativeVideoMouseWheelEvent, NativeVideoWindowEvent,
    };

    #[test]
    fn seek_strip_cursor_follows_presenter_hover_when_egui_hover_drops() {
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        let overlay_size = egui::vec2(1280.0, 720.0);
        let pointer = egui::pos2(640.0, 600.0);
        let strip = NativeOverlaySeekStrip {
            center: crate::video::seek_strip::SeekStripCenter::Thumbnails { center_index: 0.0 },
            axis: super::NativeOverlaySeekStripAxis::Ready(std::sync::Arc::new(
                crate::video::seek_strip::StripAxis::TimeGrid {
                    interval_secs: 15.0,
                    fallback_interval_secs: 15.0,
                    duration_secs: 15.0,
                },
            )),
            range_value_secs: crate::settings::VIDEO_SEEK_STRIP_MIN_INTERVAL_DEFAULT_SECS,
            cell_count: 1,
            cells: Vec::new(),
            thumbnail_notice: None,
            wave_image: None,
            wave_notice: None,
            waveform_span_secs: crate::video::seek_strip_wave::DEFAULT_WAVEFORM_SPAN_SECS,
        };
        let mut drag_origin = None;
        let mut last_window_request = None;

        let presenter_hover_positions = [Some(pointer), Some(pointer), Some(pointer), None];
        let mut egui_hover_positions = Vec::new();
        let mut cursor_icons = Vec::new();
        for frame in 0..4 {
            let mut commands = Vec::new();
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, overlay_size)),
                    time: Some(frame as f64 / 60.0),
                    events: match frame {
                        0 => vec![egui::Event::PointerMoved(pointer)],
                        2 => vec![egui::Event::PointerGone],
                        _ => Vec::new(),
                    },
                    ..Default::default()
                },
                |ctx| {
                    draw_native_seek_strip(
                        ctx,
                        overlay_size,
                        presenter_hover_positions[frame],
                        true,
                        &strip,
                        false,
                        &std::collections::HashMap::new(),
                        None,
                        &mut drag_origin,
                        &mut last_window_request,
                        &mut commands,
                    );
                    egui::Area::new(egui::Id::new("native_video_seek_hud"))
                        .order(egui::Order::Foreground)
                        .fixed_pos(egui::pos2(0.0, overlay_size.y - HUD_BOTTOM_HEIGHT))
                        .show(ctx, |ui| {
                            ui.set_min_size(egui::vec2(overlay_size.x, HUD_BOTTOM_HEIGHT));
                        });
                },
            );
            let egui_hover = ctx.pointer_hover_pos();
            egui_hover_positions.push(egui_hover);
            cursor_icons.push(output.platform_output.cursor_icon);
        }

        assert_eq!(
            egui_hover_positions,
            vec![Some(pointer), Some(pointer), None, None]
        );
        assert_eq!(
            cursor_icons,
            vec![
                egui::CursorIcon::ResizeHorizontal,
                egui::CursorIcon::ResizeHorizontal,
                egui::CursorIcon::ResizeHorizontal,
                egui::CursorIcon::Default,
            ],
            "presenter hover must own the strip cursor until the native position leaves"
        );
    }

    #[test]
    fn seek_preview_anchor_switches_to_strip_top_without_overlap() {
        let overlay_size = egui::vec2(1920.0, 1080.0);
        let hud_rect = egui::Rect::from_min_size(
            egui::pos2(0.0, overlay_size.y - HUD_BOTTOM_HEIGHT),
            egui::vec2(overlay_size.x, HUD_BOTTOM_HEIGHT),
        );
        let strip_rect = super::native_seek_strip_rect(overlay_size.x, overlay_size.y);
        let without_strip =
            super::native_seek_preview_layout(overlay_size.x, overlay_size.x * 0.5, hud_rect, None);
        let with_strip = super::native_seek_preview_layout(
            overlay_size.x,
            overlay_size.x * 0.5,
            hud_rect,
            Some(strip_rect),
        );

        assert_eq!(
            without_strip.preview_rect.max.y,
            hud_rect.min.y - super::SEEK_PREVIEW_GAP
        );
        assert_eq!(
            with_strip.preview_rect.max.y,
            strip_rect.min.y - super::SEEK_PREVIEW_GAP
        );
        assert!(with_strip.preview_rect.max.y < strip_rect.min.y);
        assert!(!with_strip.preview_rect.intersects(strip_rect));
        assert!(with_strip.preview_rect.min.y < without_strip.preview_rect.min.y);

        // Short viewports shrink the image instead of pushing the preview down across the strip.
        let short_size = egui::vec2(640.0, 360.0);
        let short_hud = egui::Rect::from_min_size(
            egui::pos2(0.0, short_size.y - HUD_BOTTOM_HEIGHT),
            egui::vec2(short_size.x, HUD_BOTTOM_HEIGHT),
        );
        let short_strip = super::native_seek_strip_rect(short_size.x, short_size.y);
        let short_layout = super::native_seek_preview_layout(
            short_size.x,
            short_size.x * 0.5,
            short_hud,
            Some(short_strip),
        );
        assert_eq!(
            short_layout.preview_rect.max.y,
            short_strip.min.y - super::SEEK_PREVIEW_GAP
        );
        assert!(!short_layout.preview_rect.intersects(short_strip));
    }

    /// 実機報告 2026-08-26: ストリップを hover するとプレビューがポインタから離れた場所に出て、
    /// 左端に寄ったり右へずれたりした。x を時刻からシークバー上の位置へ引き直していたため。
    /// ストリップはキーフレーム番号軸で再生位置中心なので、その x は時間線形の x と一致しない。
    #[test]
    fn the_preview_follows_whichever_surface_the_pointer_is_over() {
        let mut target = None;
        let mut anchor = None;

        // ストリップ: 指している x をそのまま使う。同じ時刻でもシークバー上の位置とは違う。
        super::set_seek_preview_target(&mut target, &mut anchor, Some((250.0, 180.0)));
        assert_eq!(target, Some(250.0));
        assert_eq!(anchor, Some(180.0));

        // シークバー: バー上で clamp した x を使う。
        super::set_seek_preview_target(&mut target, &mut anchor, Some((250.0, 1500.0)));
        assert_eq!(anchor, Some(1500.0));

        // 片方だけ残らない。
        super::set_seek_preview_target(&mut target, &mut anchor, None);
        assert_eq!((target, anchor), (None, None));
    }

    #[test]
    fn the_preview_is_centred_on_its_anchor() {
        let overlay_width = 1920.0;
        let hud_rect = egui::Rect::from_min_size(
            egui::pos2(0.0, 1080.0 - HUD_BOTTOM_HEIGHT),
            egui::vec2(overlay_width, HUD_BOTTOM_HEIGHT),
        );
        let left = super::native_seek_preview_layout(overlay_width, 400.0, hud_rect, None);
        let right = super::native_seek_preview_layout(overlay_width, 1500.0, hud_rect, None);
        assert_eq!(left.preview_rect.center().x, 400.0);
        assert_eq!(right.preview_rect.center().x, 1500.0);
        assert!(left.preview_rect.center().x < right.preview_rect.center().x);
    }

    #[test]
    fn seek_strip_thumbnail_preview_uses_fractional_axis_time() {
        let rect = super::native_seek_strip_rect(1280.0, 720.0);
        let strip = NativeOverlaySeekStrip {
            center: crate::video::seek_strip::SeekStripCenter::Thumbnails { center_index: 1.0 },
            axis: super::NativeOverlaySeekStripAxis::Ready(std::sync::Arc::new(
                crate::video::seek_strip::StripAxis::KeyframeIndex {
                    keyframes: vec![0.0, 10.0, 40.0],
                    adopted: vec![0, 1, 2],
                },
            )),
            range_value_secs: 15.0,
            cell_count: 3,
            cells: Vec::new(),
            thumbnail_notice: None,
            wave_image: None,
            wave_notice: None,
            waveform_span_secs: 120.0,
        };
        let pointer = egui::pos2(
            rect.center().x + super::SEEK_STRIP_CELL_WIDTH * 0.5,
            rect.center().y,
        );
        assert_eq!(
            super::native_seek_strip_preview_hover(&strip, rect, pointer, true),
            super::NativeSeekStripPreviewHover::Target(25.0)
        );
        let drag_pointer = egui::pos2(pointer.x, rect.min.y - 20.0);
        assert_eq!(
            super::native_seek_strip_preview_hover(&strip, rect, drag_pointer, true),
            super::NativeSeekStripPreviewHover::Target(25.0)
        );
    }

    #[test]
    fn seek_strip_wave_preview_uses_pointer_time_but_lock_suppresses_it() {
        let rect = super::native_seek_strip_rect(1280.0, 720.0);
        let strip = NativeOverlaySeekStrip {
            center: crate::video::seek_strip::SeekStripCenter::Waveform {
                center_time_secs: 60.0,
            },
            axis: super::NativeOverlaySeekStripAxis::Resolving,
            range_value_secs: 120.0,
            cell_count: 0,
            cells: Vec::new(),
            thumbnail_notice: None,
            wave_image: None,
            wave_notice: None,
            waveform_span_secs: 120.0,
        };
        let pointer = egui::pos2(rect.center().x + rect.width() * 0.25, rect.center().y);
        assert_eq!(
            super::native_seek_strip_preview_hover(&strip, rect, pointer, true),
            super::NativeSeekStripPreviewHover::Target(90.0)
        );
        assert_eq!(
            super::native_seek_strip_preview_hover(
                &strip,
                rect,
                super::native_seek_strip_lock_button_rect(rect).center(),
                true,
            ),
            super::NativeSeekStripPreviewHover::Suppress
        );
    }

    #[test]
    fn seek_strip_wheel_is_consumed_and_becomes_one_range_step() {
        let overlay_size = egui::vec2(1280.0, 720.0);
        let strip_rect = super::native_seek_strip_rect(overlay_size.x, overlay_size.y);
        // Over the plain band and over the lock button, which sits inside the same rect and
        // must not swallow the wheel.
        for pointer in [
            strip_rect.center(),
            super::native_seek_strip_lock_button_rect(strip_rect).center(),
        ] {
            let ctx = egui::Context::default();
            crate::ui_fonts::configure_fonts(&ctx);
            let strip = NativeOverlaySeekStrip {
                center: crate::video::seek_strip::SeekStripCenter::Thumbnails { center_index: 0.0 },
                axis: super::NativeOverlaySeekStripAxis::Ready(std::sync::Arc::new(
                    crate::video::seek_strip::StripAxis::TimeGrid {
                        interval_secs: 15.0,
                        fallback_interval_secs: 15.0,
                        duration_secs: 15.0,
                    },
                )),
                range_value_secs: 15.0,
                cell_count: 1,
                cells: Vec::new(),
                thumbnail_notice: None,
                wave_image: None,
                wave_notice: None,
                waveform_span_secs: crate::video::seek_strip_wave::DEFAULT_WAVEFORM_SPAN_SECS,
            };
            let mut drag_origin = None;
            let mut last_window_request = None;
            let mut commands = Vec::new();
            let mut wheel_remaining = true;
            assert!(
                immediate_native_wheel_command(120, false, false, true, false, false).is_none(),
                "wheel over the strip must not enqueue navigation before egui handles it"
            );
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, overlay_size)),
                    events: vec![egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Line,
                        delta: egui::vec2(0.0, 1.0),
                        modifiers: egui::Modifiers::default(),
                    }],
                    ..Default::default()
                },
                |ctx| {
                    draw_native_seek_strip(
                        ctx,
                        overlay_size,
                        Some(pointer),
                        true,
                        &strip,
                        false,
                        &std::collections::HashMap::new(),
                        None,
                        &mut drag_origin,
                        &mut last_window_request,
                        &mut commands,
                    );
                    wheel_remaining = ctx.input(|input| {
                        input.raw_scroll_delta != egui::Vec2::ZERO
                            || input.smooth_scroll_delta != egui::Vec2::ZERO
                            || input
                                .events
                                .iter()
                                .any(|event| matches!(event, egui::Event::MouseWheel { .. }))
                    });
                },
            );

            assert!(!wheel_remaining);
            assert!(commands.iter().any(|command| matches!(
                command,
                super::NativeOverlayCommand::StepSeekStripRange {
                    step: crate::video::seek_strip::SeekStripRangeStep::Narrower
                }
            )));
            assert!(!commands.iter().any(|command| matches!(
                command,
                super::NativeOverlayCommand::NavigateItem { .. }
            )));
        }
    }

    #[test]
    fn seek_strip_lock_layout_keeps_the_button_out_of_the_seek_body() {
        let overlay_size = egui::vec2(1280.0, 720.0);
        let strip_rect = super::native_seek_strip_rect(overlay_size.x, overlay_size.y);
        let lock_rect = super::native_seek_strip_lock_button_rect(strip_rect);
        let range_pos = super::native_seek_strip_range_text_pos(strip_rect);
        assert_eq!(lock_rect.min, egui::pos2(1245.0, 556.0));
        assert_eq!(lock_rect.max, egui::pos2(1273.0, 584.0));
        assert_eq!(range_pos, egui::pos2(1239.0, 557.0));
        assert!(!super::seek_strip_body_accepts_pointer(
            lock_rect,
            lock_rect.center()
        ));
        assert!(super::seek_strip_body_accepts_pointer(
            lock_rect,
            egui::pos2(640.0, 600.0)
        ));
    }

    #[test]
    fn seek_strip_lock_button_click_emits_only_the_lock_command() {
        use std::sync::{Arc, Mutex};

        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_for_ui = Arc::clone(&captured);
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(80.0, 80.0))
            .build(move |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let painter = ui.painter().clone();
                    let mut commands = Vec::new();
                    super::draw_native_seek_strip_lock_button(
                        ui,
                        &painter,
                        egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(28.0, 28.0)),
                        false,
                        &mut commands,
                    );
                    captured_for_ui.lock().unwrap().extend(commands);
                });
            });
        harness.run();
        let center = egui::pos2(24.0, 24.0);
        harness.hover_at(center);
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos: center,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        harness.run();

        let commands = captured.lock().unwrap();
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, super::NativeOverlayCommand::ToggleSeekStripLock))
        );
        assert_eq!(commands.len(), 1);
    }

    /// The isolated button test above only proves the button emits its own command. This one
    /// drives the whole strip, because the strip body owns a `click_and_drag` interact over the
    /// same rect: a click on the lock must reach the lock and must not also commit a seek.
    #[test]
    fn a_click_in_the_strip_goes_to_the_lock_or_the_seek_body_but_never_both() {
        use std::sync::{Arc, Mutex};

        let overlay_size = egui::vec2(1280.0, 720.0);
        let strip_rect = super::native_seek_strip_rect(overlay_size.x, overlay_size.y);
        let lock_center = super::native_seek_strip_lock_button_rect(strip_rect).center();
        let body_center = strip_rect.center();

        let click_at = |pointer: egui::Pos2| {
            let captured = Arc::new(Mutex::new(Vec::new()));
            let captured_for_ui = Arc::clone(&captured);
            let drag_origin = Arc::new(Mutex::new(None));
            let last_window_request = Arc::new(Mutex::new(None));
            let mut fonts_ready = false;
            let mut harness = egui_kittest::Harness::builder()
                .with_size(overlay_size)
                .build(move |ctx| {
                    if !fonts_ready {
                        crate::ui_fonts::configure_fonts(ctx);
                        fonts_ready = true;
                        ctx.request_repaint();
                        return;
                    }
                    let strip = NativeOverlaySeekStrip {
                        center: crate::video::seek_strip::SeekStripCenter::Thumbnails {
                            center_index: 4.0,
                        },
                        axis: super::NativeOverlaySeekStripAxis::Ready(std::sync::Arc::new(
                            crate::video::seek_strip::StripAxis::TimeGrid {
                                interval_secs: 15.0,
                                fallback_interval_secs: 15.0,
                                duration_secs: 480.0,
                            },
                        )),
                        range_value_secs:
                            crate::settings::VIDEO_SEEK_STRIP_MIN_INTERVAL_DEFAULT_SECS,
                        cell_count: 32,
                        cells: Vec::new(),
                        thumbnail_notice: None,
                        wave_image: None,
                        wave_notice: None,
                        waveform_span_secs:
                            crate::video::seek_strip_wave::DEFAULT_WAVEFORM_SPAN_SECS,
                    };
                    let mut commands = Vec::new();
                    draw_native_seek_strip(
                        ctx,
                        overlay_size,
                        Some(pointer),
                        true,
                        &strip,
                        false,
                        &std::collections::HashMap::new(),
                        None,
                        &mut drag_origin.lock().unwrap(),
                        &mut last_window_request.lock().unwrap(),
                        &mut commands,
                    );
                    captured_for_ui.lock().unwrap().extend(commands);
                });
            harness.run();
            harness.hover_at(pointer);
            for pressed in [true, false] {
                harness.event(egui::Event::PointerButton {
                    pos: pointer,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            harness.run();

            let commands = captured.lock().unwrap();
            let locked = commands
                .iter()
                .any(|command| matches!(command, super::NativeOverlayCommand::ToggleSeekStripLock));
            let seeked = commands.iter().any(|command| {
                matches!(command, super::NativeOverlayCommand::CommitSeekStrip { .. })
            });
            (locked, seeked)
        };

        assert_eq!(
            click_at(lock_center),
            (true, false),
            "a click on the lock must toggle the lock and must not commit a seek"
        );
        assert_eq!(
            click_at(body_center),
            (false, true),
            "a click on the strip body must still commit a seek"
        );
    }

    /// Repro for the real-hardware report: with the strip on screen, the bottom HUD's lock
    /// needed two clicks. The logs showed the widget reporting `contains_pointer=false` while
    /// the pointer sat inside its rect, and the correlation across two sessions was exact -
    /// suppressed whenever the strip was present, fine whenever it was not.
    #[test]
    fn the_bottom_hud_lock_is_clickable_while_the_strip_is_on_screen() {
        use std::sync::{Arc, Mutex};

        // Geometry from the real-hardware log: fractional points size at 1.5x scale.
        let overlay_size = egui::vec2(1242.7, 916.0);
        let lock_rect =
            crate::video::native_presenter::overlay_draw::native_seek_bar_lock_button_rect(
                overlay_size.x,
                overlay_size.y,
            );
        let pointer = lock_rect.center();

        let run = |with_strip: bool| {
            let captured: Arc<Mutex<Vec<super::NativeOverlayCommand>>> =
                Arc::new(Mutex::new(Vec::new()));
            let captured_for_ui = Arc::clone(&captured);
            let drag_origin = Arc::new(Mutex::new(None));
            let last_window_request = Arc::new(Mutex::new(None));
            let mut fonts_ready = false;
            let mut harness = egui_kittest::Harness::builder()
                .with_size(overlay_size)
                .with_pixels_per_point(1.5)
                .build(move |ctx| {
                    if !fonts_ready {
                        crate::ui_fonts::configure_fonts(ctx);
                        fonts_ready = true;
                        ctx.request_repaint();
                        return;
                    }
                    let mut commands = Vec::new();
                    if with_strip {
                        let strip = NativeOverlaySeekStrip {
                            center: crate::video::seek_strip::SeekStripCenter::Thumbnails {
                                center_index: 4.0,
                            },
                            axis: super::NativeOverlaySeekStripAxis::Ready(std::sync::Arc::new(
                                crate::video::seek_strip::StripAxis::TimeGrid {
                                    interval_secs: 15.0,
                                    fallback_interval_secs: 15.0,
                                    duration_secs: 480.0,
                                },
                            )),
                            range_value_secs:
                                crate::settings::VIDEO_SEEK_STRIP_MIN_INTERVAL_DEFAULT_SECS,
                            cell_count: 32,
                            cells: Vec::new(),
                            thumbnail_notice: None,
                            wave_image: None,
                            wave_notice: None,
                            waveform_span_secs:
                                crate::video::seek_strip_wave::DEFAULT_WAVEFORM_SPAN_SECS,
                        };
                        draw_native_seek_strip(
                            ctx,
                            overlay_size,
                            Some(pointer),
                            true,
                            &strip,
                            false,
                            &std::collections::HashMap::new(),
                            None,
                            &mut drag_origin.lock().unwrap(),
                            &mut last_window_request.lock().unwrap(),
                            &mut commands,
                        );
                    }
                    egui::Area::new(egui::Id::new("native_video_seek_hud"))
                        .order(egui::Order::Foreground)
                        .fixed_pos(egui::pos2(0.0, overlay_size.y - HUD_BOTTOM_HEIGHT))
                        .show(ctx, |ui| {
                            ui.set_min_size(egui::vec2(overlay_size.x, HUD_BOTTOM_HEIGHT));
                            let painter = ui.painter().clone();
                            crate::video::native_presenter::overlay_draw::
                                draw_native_bar_lock_button(
                                    ui,
                                    &painter,
                                    lock_rect,
                                    "native_seek_bar_lock",
                                    true,
                                    "シークバー固定を解除",
                                    crate::video::NativeVideoBar::Seek,
                                    &mut commands,
                                );
                        });
                    captured_for_ui.lock().unwrap().extend(commands);
                });
            harness.run();
            // Reach the HUD lock the way the report did: click the strip's own lock first,
            // then move down to the bar's lock and click once.
            if with_strip {
                let strip_lock = super::native_seek_strip_lock_button_rect(
                    super::native_seek_strip_rect(overlay_size.x, overlay_size.y),
                )
                .center();
                harness.hover_at(strip_lock);
                harness.run();
                for pressed in [true, false] {
                    harness.event(egui::Event::PointerButton {
                        pos: strip_lock,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    });
                }
                harness.run();
            }
            harness.hover_at(pointer);
            harness.run();
            for pressed in [true, false] {
                harness.event(egui::Event::PointerButton {
                    pos: pointer,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            harness.run();
            let commands = captured.lock().unwrap();
            commands
                .iter()
                .filter(|command| {
                    matches!(command, super::NativeOverlayCommand::ToggleBarLock { .. })
                })
                .count()
        };

        assert_eq!(
            run(false),
            1,
            "sanity: the lock works with no strip on screen"
        );
        assert_eq!(
            run(true),
            1,
            "one click on the bottom HUD lock must still toggle it while the strip is showing"
        );
    }

    /// Real-hardware report: while a toast was on screen, the first click on the bottom HUD's
    /// lock did nothing and only the second one landed. Every egui `Area` registers a widget at
    /// its own rect - `interactable(false)` only downgrades the sense to hover, it does not stop
    /// the registration - and egui's hit test drops a widget that a front widget in another layer
    /// fully contains. A passive overlay sized to the whole screen therefore swallows every HUD
    /// button under it for as long as it shows.
    #[test]
    fn a_toast_does_not_swallow_clicks_on_the_hud_beneath_it() {
        use egui_kittest::Harness;
        use std::sync::{Arc, Mutex};

        let overlay_size = egui::vec2(1242.7, 916.0);
        let lock_rect =
            crate::video::native_presenter::overlay_draw::native_seek_bar_lock_button_rect(
                overlay_size.x,
                overlay_size.y,
            );
        let pointer = lock_rect.center();

        // Both passive overlays, so the pair is covered rather than only the one reported. The
        // tile grid, the navigation preview and the normalize blocker are meant to cover the
        // screen and are not in this list.
        #[derive(Clone, Copy)]
        enum Passive {
            None,
            Toast,
            CenterStatus,
        }

        let clicks_with_overlay = |passive: Passive| {
            let captured = Arc::new(Mutex::new(Vec::new()));
            let captured_for_ui = Arc::clone(&captured);
            let mut fonts_ready = false;
            let mut harness = Harness::builder()
                .with_size(overlay_size)
                .with_pixels_per_point(1.5)
                .build(move |ctx| {
                    if !fonts_ready {
                        crate::ui_fonts::configure_fonts(ctx);
                        fonts_ready = true;
                        ctx.request_repaint();
                        return;
                    }
                    let mut commands = Vec::new();
                    egui::Area::new(egui::Id::new("native_video_seek_hud"))
                        .order(egui::Order::Foreground)
                        .fixed_pos(egui::pos2(
                            0.0,
                            overlay_size.y - HUD_BOTTOM_HEIGHT,
                        ))
                        .show(ctx, |ui| {
                            ui.set_min_size(egui::vec2(
                                overlay_size.x,
                                HUD_BOTTOM_HEIGHT,
                            ));
                            let painter = ui.painter().clone();
                            crate::video::native_presenter::overlay_draw::draw_native_bar_lock_button(
                                ui,
                                &painter,
                                lock_rect,
                                "native_seek_bar_lock",
                                true,
                                "シークバー固定を解除",
                                crate::video::NativeVideoBar::Seek,
                                &mut commands,
                            );
                        });
                    // Drawn after the HUD, so it is the frontmost Foreground area - exactly the
                    // order the presenter produces right after a lock is toggled.
                    match passive {
                        Passive::None => {}
                        Passive::Toast => {
                            crate::video::native_presenter::overlay_draw::draw_native_toast(
                                ctx,
                                overlay_size.x,
                                overlay_size.y,
                                &super::NativeOverlayToast::for_test("下部シークバー: 固定"),
                            );
                        }
                        Passive::CenterStatus => {
                            crate::video::native_presenter::overlay_draw::draw_native_center_status(
                                ctx,
                                overlay_size.x,
                                overlay_size.y,
                                "読み込み中",
                                None,
                                false,
                            );
                        }
                    }
                    captured_for_ui.lock().unwrap().extend(commands);
                });
            harness.run();
            harness.hover_at(pointer);
            harness.run();
            for pressed in [true, false] {
                harness.event(egui::Event::PointerButton {
                    pos: pointer,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            harness.run();
            let commands = captured.lock().unwrap();
            commands
                .iter()
                .filter(|command| {
                    matches!(command, super::NativeOverlayCommand::ToggleBarLock { .. })
                })
                .count()
        };

        assert_eq!(
            clicks_with_overlay(Passive::None),
            1,
            "sanity: the lock works with nothing over it"
        );
        assert_eq!(
            clicks_with_overlay(Passive::Toast),
            1,
            "a toast must not eat the first click on the HUD button under it"
        );
        assert_eq!(
            clicks_with_overlay(Passive::CenterStatus),
            1,
            "the centre status box must not eat it either"
        );
    }

    #[test]
    fn first_present_transition_does_not_invalidate_the_prepared_scale_signature() {
        let signature = VideoScalePreparationSignature {
            source_width: 1920,
            source_height: 1080,
            sar_num: 1,
            sar_den: 1,
            orientation: VideoOrientation::IDENTITY,
            target_width: 3840,
            target_height: 2160,
            target_content: VideoSurfaceContent::DisplayResolved,
            panorama_active: false,
            grade_active: false,
            filter: crate::settings::VideoScaleFilter::Sharp,
            smoothing_percent: 30,
            anime4k_variant: None,
        };
        let identity = crate::video::VideoScaleRequestIdentity {
            source_epoch: 11,
            revision: 7,
        };
        let prepared = Some(PreparedVideoScaleSettings {
            identity,
            signature,
        });
        assert_eq!(
            validate_prepared_video_scale_settings(prepared, identity, signature),
            Ok(())
        );
        // The first present changes current_surface from LegacySource to
        // DisplayResolved. Transition state is intentionally absent here, so
        // that mutation cannot invalidate an otherwise identical resource key.
        assert!(video_scale_signature_changed_fields(signature, signature).is_empty());

        let changed_geometry = VideoScalePreparationSignature {
            target_width: 2560,
            target_height: 1440,
            ..signature
        };
        assert_eq!(
            validate_prepared_video_scale_settings(prepared, identity, changed_geometry),
            Err(
                crate::video::VideoScaleApplyRejectReason::SignatureChanged {
                    fields: vec!["target_width", "target_height"],
                }
            )
        );

        assert_eq!(
            validate_prepared_video_scale_settings(
                prepared,
                crate::video::VideoScaleRequestIdentity {
                    source_epoch: 12,
                    revision: 7,
                },
                signature,
            ),
            Err(crate::video::VideoScaleApplyRejectReason::SourceChanged {
                prepared_source_epoch: 11,
                current_source_epoch: 12,
            })
        );
        assert_eq!(
            validate_prepared_video_scale_settings(
                prepared,
                crate::video::VideoScaleRequestIdentity {
                    source_epoch: 11,
                    revision: 8,
                },
                signature,
            ),
            Err(crate::video::VideoScaleApplyRejectReason::RequestMismatch {
                prepared_revision: 7,
                current_revision: 8,
            })
        );
    }
    fn key(virtual_key: u32) -> NativeVideoKeyEvent {
        NativeVideoKeyEvent {
            virtual_key,
            scan_code: 0,
            extended: false,
            shift: false,
            ctrl: false,
            alt: false,
            repeat: false,
        }
    }

    fn wheel(ctrl: bool) -> NativeVideoWindowEvent {
        NativeVideoWindowEvent::MouseWheel(NativeVideoMouseWheelEvent {
            delta: 120,
            x: 100,
            y: 100,
            shift: false,
            ctrl,
        })
    }

    fn mouse_button(button: NativeVideoMouseButton) -> NativeVideoWindowEvent {
        NativeVideoWindowEvent::MouseButton(NativeVideoMouseButtonEvent {
            button,
            down: true,
            double_click: false,
            x: 100,
            y: 100,
            shift: false,
            ctrl: false,
        })
    }

    fn mouse_move() -> NativeVideoWindowEvent {
        NativeVideoWindowEvent::MouseMove(NativeVideoMouseEvent {
            x: 100,
            y: 100,
            shift: false,
            ctrl: false,
        })
    }

    #[test]
    fn native_overlay_uses_shared_strong_dark_text_style() {
        let ctx = egui::Context::default();
        configure_overlay_style(&ctx, crate::settings::TextContrast::Strong);

        assert!(ctx.style().visuals.dark_mode);
        assert_eq!(
            crate::os_theme::current_text_contrast(&ctx),
            crate::settings::TextContrast::Strong
        );
        assert_eq!(ctx.style().interaction.tooltip_delay, 0.0);
        assert_eq!(
            ctx.style().visuals.text_color(),
            crate::os_theme::dark_visuals(crate::settings::TextContrast::Strong).text_color()
        );

        configure_overlay_style(&ctx, crate::settings::TextContrast::Standard);
        assert_eq!(
            crate::os_theme::current_text_contrast(&ctx),
            crate::settings::TextContrast::Standard
        );
        assert_eq!(
            ctx.style().visuals.text_color(),
            crate::os_theme::dark_visuals(crate::settings::TextContrast::Standard).text_color()
        );
    }

    #[test]
    fn retained_frame_releases_reader_key_without_rearm_acquire() {
        let source = include_str!("render_core.rs");
        let forbidden_rearm_acquire = ["Acquire", "Sync(0"].concat();
        assert!(source.contains("self.mutex.ReleaseSync(1)"));
        assert!(!source.contains(&forbidden_rearm_acquire));
    }

    #[test]
    fn presenter_ppp_is_os_dpi_times_ui_scale() {
        for (os_ppp, ui_scale, expected) in [
            (1.0, 0.5, 0.5),
            (1.0, 1.0, 1.0),
            (1.25, 1.2, 1.5),
            (1.5, 2.0, 3.0),
        ] {
            let actual = effective_overlay_pixels_per_point(os_ppp, ui_scale);
            assert!((actual - expected).abs() < 1.0e-6);
        }
    }

    #[test]
    fn raw_ppp_floor_removal_is_noop_at_or_above_one() {
        for ppp in [1.0_f32, 1.25, 1.5, 2.0, 4.0] {
            assert_eq!(ppp, ppp.max(1.0));
        }
        assert_eq!(effective_overlay_pixels_per_point(1.0, 0.5), 0.5);
        assert_ne!(0.5_f32, 0.5_f32.max(1.0));
    }

    #[test]
    fn native_overlay_does_not_double_forward_consumed_wheel() {
        // overlay が wheel を NavigateItem / TileColumnsDelta コマンドへ変換済みなら、
        // wants_pointer_input が false でも raw wheel を App へ転送しない
        // (= overlay コマンドと App 側 wheel ハンドラの二重適用を防ぐ)。
        let consumed = NativeOverlayInputRouting {
            wants_pointer_input: false,
            consumed_wheel: true,
            ..Default::default()
        };
        assert!(!consumed.should_forward_to_ui(&wheel(true)));
        assert!(!consumed.should_forward_to_ui(&wheel(false)));

        // 未消費 (= overlay 無効のフォールバック経路など) なら従来どおり
        // wants_pointer_input 次第で転送する。
        let fallback = NativeOverlayInputRouting {
            wants_pointer_input: false,
            consumed_wheel: false,
            ..Default::default()
        };
        assert!(fallback.should_forward_to_ui(&wheel(true)));

        // overlay UI 上 (wants_pointer_input=true) なら未消費でも転送しない。
        let over_ui = NativeOverlayInputRouting {
            wants_pointer_input: true,
            consumed_wheel: false,
            ..Default::default()
        };
        assert!(!over_ui.should_forward_to_ui(&wheel(true)));
        assert!(!over_ui.should_forward_to_ui(&wheel(false)));
    }

    #[test]
    fn native_overlay_routes_right_click_close_over_hud() {
        let over_ui = NativeOverlayInputRouting {
            wants_pointer_input: true,
            ..Default::default()
        };
        assert!(over_ui.should_forward_to_ui(&mouse_button(NativeVideoMouseButton::Right)));
        assert!(!over_ui.should_forward_to_ui(&mouse_button(NativeVideoMouseButton::Left)));

        let modal = NativeOverlayInputRouting {
            wants_pointer_input: true,
            modal_dialog_active: true,
            ..Default::default()
        };
        assert!(!modal.should_forward_to_ui(&mouse_button(NativeVideoMouseButton::Right)));
        assert!(!modal.should_forward_to_ui(&mouse_button(NativeVideoMouseButton::Left)));
        assert!(!modal.should_forward_to_ui(&wheel(false)));
        assert!(!modal.should_forward_to_ui(&NativeVideoWindowEvent::Text('?')));
        assert!(!modal.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0x20))));
    }

    #[test]
    fn parked_dimmed_hud_forwards_raw_mouse_buttons_but_not_wheel() {
        let parked_over_ui = NativeOverlayInputRouting {
            wants_pointer_input: true,
            modal_dialog_active: true,
            hud_dimmed: true,
            ..Default::default()
        };

        assert!(parked_over_ui.should_forward_to_ui(&mouse_button(NativeVideoMouseButton::Left)));
        assert!(parked_over_ui.should_forward_to_ui(&mouse_button(NativeVideoMouseButton::Right)));
        assert!(
            !parked_over_ui.should_forward_to_ui(&wheel(false)),
            "parked passthrough is limited to raw mouse buttons; wheel remains inert"
        );
    }

    #[test]
    fn dimmed_hud_suppresses_pointer_delivery_to_overlay_egui() {
        assert!(NativeEguiOverlay::hud_dimmed_suppresses_overlay_pointer_event(&mouse_move()));
        assert!(
            NativeEguiOverlay::hud_dimmed_suppresses_overlay_pointer_event(&mouse_button(
                NativeVideoMouseButton::Left
            ))
        );
        assert!(NativeEguiOverlay::hud_dimmed_suppresses_overlay_pointer_event(&wheel(false)));
        assert!(
            NativeEguiOverlay::hud_dimmed_suppresses_overlay_pointer_event(
                &NativeVideoWindowEvent::MouseLeave
            )
        );
        assert!(
            !NativeEguiOverlay::hud_dimmed_suppresses_overlay_pointer_event(
                &NativeVideoWindowEvent::KeyDown(key(0x20))
            )
        );
        assert!(
            !NativeEguiOverlay::hud_dimmed_suppresses_overlay_pointer_event(
                &NativeVideoWindowEvent::CloseRequested { generation: 7 }
            )
        );
    }

    #[test]
    fn dimmed_hud_visibility_uses_raw_hover_without_overlay_pointer_delivery() {
        let bottom_hover = Some(egui::pos2(120.0, 650.0));
        let upper_hover = Some(egui::pos2(120.0, 100.0));

        assert!(
            NativeEguiOverlay::native_hud_bottom_visible_from_hover(
                bottom_hover,
                720.0,
                false,
                false,
                false,
            ),
            "raw hover near the bottom should show the dimmed HUD"
        );
        assert!(!NativeEguiOverlay::native_hud_bottom_visible_from_hover(
            upper_hover,
            720.0,
            false,
            false,
            false,
        ));
        assert!(
            !NativeEguiOverlay::native_hud_bottom_visible_from_hover(
                bottom_hover,
                720.0,
                true,
                false,
                false,
            ),
            "external drag remains authoritative even for raw hover"
        );
        assert!(
            NativeEguiOverlay::hud_dimmed_suppresses_overlay_pointer_event(&mouse_move()),
            "raw hover visibility must not re-enable egui pointer delivery while dimmed"
        );
    }

    #[test]
    fn dimmed_top_bar_visibility_uses_raw_hover_without_overlay_pointer_delivery() {
        assert!(NativeEguiOverlay::native_hud_top_visible_from_hover(
            Some(egui::pos2(40.0, 20.0)),
            false,
            false,
            false,
            false,
        ));
        assert!(!NativeEguiOverlay::native_hud_top_visible_from_hover(
            Some(egui::pos2(40.0, 80.0)),
            false,
            false,
            false,
            false,
        ));
        assert!(
            NativeEguiOverlay::hud_dimmed_suppresses_overlay_pointer_event(
                &NativeVideoWindowEvent::MouseLeave
            )
        );
    }

    #[test]
    fn undimming_hud_hands_raw_hover_back_to_the_active_overlay() {
        let bottom_hover = egui::pos2(120.0, 650.0);
        let mut pointer_pos = None;
        let mut raw_hover_pos = Some(bottom_hover);

        let moved = NativeEguiOverlay::restore_active_hud_hover_after_undim(
            &mut pointer_pos,
            &mut raw_hover_pos,
        );

        assert_eq!(moved, Some(bottom_hover));
        assert_eq!(pointer_pos, Some(bottom_hover));
        assert_eq!(raw_hover_pos, None);
        assert!(NativeEguiOverlay::native_hud_bottom_visible_from_hover(
            pointer_pos,
            720.0,
            false,
            false,
            false,
        ));
    }

    #[test]
    fn touch_chrome_latch_reveals_bars_without_hover_and_preserves_hover_behavior() {
        assert!(NativeEguiOverlay::native_hud_bottom_visible_from_hover(
            None, 720.0, false, true, false,
        ));
        assert!(NativeEguiOverlay::native_hud_top_visible_from_hover(
            None, false, false, true, false,
        ));
        assert!(NativeEguiOverlay::native_hud_bottom_visible_from_hover(
            Some(egui::pos2(100.0, 650.0)),
            720.0,
            false,
            false,
            false,
        ));
        assert!(!NativeEguiOverlay::native_hud_bottom_visible_from_hover(
            Some(egui::pos2(100.0, 100.0)),
            720.0,
            false,
            false,
            false,
        ));
    }

    #[test]
    fn locked_video_bars_are_independent_inputs_to_visibility_functions() {
        for &(top_locked, seek_locked, expected_top, expected_bottom) in &[
            (false, false, false, false),
            (true, false, true, false),
            (false, true, false, true),
            (true, true, true, true),
        ] {
            assert_eq!(
                NativeEguiOverlay::native_hud_top_visible_from_hover(
                    None, false, false, false, top_locked,
                ),
                expected_top
            );
            assert_eq!(
                NativeEguiOverlay::native_hud_bottom_visible_from_hover(
                    None,
                    720.0,
                    false,
                    false,
                    seek_locked,
                ),
                expected_bottom
            );
        }

        assert!(NativeEguiOverlay::native_hud_top_visible_from_hover(
            None, false, true, false, true,
        ));
        assert!(NativeEguiOverlay::native_hud_bottom_visible_from_hover(
            None, 720.0, true, false, true,
        ));
    }

    #[test]
    fn unlocked_video_bars_return_to_existing_hover_and_touch_inputs() {
        assert!(NativeEguiOverlay::native_hud_bottom_visible_from_hover(
            None, 720.0, false, false, true,
        ));
        assert!(!NativeEguiOverlay::native_hud_bottom_visible_from_hover(
            None, 720.0, false, false, false,
        ));
        assert!(NativeEguiOverlay::native_hud_bottom_visible_from_hover(
            None, 720.0, false, true, false,
        ));

        assert!(NativeEguiOverlay::native_hud_top_visible_from_hover(
            None, false, false, false, true,
        ));
        assert!(!NativeEguiOverlay::native_hud_top_visible_from_hover(
            None, false, false, false, false,
        ));
        assert!(NativeEguiOverlay::native_hud_top_visible_from_hover(
            None, false, false, true, false,
        ));
    }

    #[test]
    fn tile_and_navigation_suppress_top_hover_chrome_snapshot() {
        let top_hover_visible = NativeEguiOverlay::native_hud_top_visible_from_hover(
            Some(egui::pos2(40.0, 20.0)),
            false,
            false,
            false,
            false,
        );
        assert!(top_hover_visible);

        let unobstructed = NativeEguiOverlay::native_bar_visibility_snapshot(
            false,
            top_hover_visible,
            top_hover_visible,
            false,
            false,
            false,
        );
        assert_eq!(
            unobstructed,
            NativeBarVisibilitySnapshot {
                top_bar_visible: true,
                bottom_hud_visible: true,
            },
            "既定 Hover では従来どおり上端 hover が上下バーを連動表示する"
        );

        for &(tile_overlay_visible, navigation_preview_visible) in &[(true, false), (false, true)] {
            let guarded = NativeEguiOverlay::native_bar_visibility_snapshot(
                false,
                top_hover_visible,
                top_hover_visible,
                false,
                tile_overlay_visible,
                navigation_preview_visible,
            );
            assert_eq!(
                guarded,
                NativeBarVisibilitySnapshot::default(),
                "tile/navigation 中の上端 hover は上下どちらの chrome にも漏らさない"
            );
        }
    }

    #[test]
    fn locked_bars_do_not_publish_tile_or_navigation_region_snapshots() {
        let locked_top_visible =
            NativeEguiOverlay::native_hud_top_visible_from_hover(None, false, false, false, true);
        assert!(locked_top_visible);

        let unobstructed = NativeEguiOverlay::native_bar_visibility_snapshot(
            false,
            locked_top_visible,
            false,
            false,
            false,
            false,
        );
        assert_eq!(
            unobstructed,
            NativeBarVisibilitySnapshot {
                top_bar_visible: true,
                bottom_hud_visible: false,
            },
            "上部固定は上部だけを描画し、下部へ固定状態を漏らさない"
        );

        for &(tile_overlay_visible, navigation_preview_visible) in &[(true, false), (false, true)] {
            let guarded = NativeEguiOverlay::native_bar_visibility_snapshot(
                false,
                locked_top_visible,
                false,
                false,
                tile_overlay_visible,
                navigation_preview_visible,
            );
            assert_eq!(
                guarded,
                NativeBarVisibilitySnapshot::default(),
                "region が読む実描画 snapshot は tile/navigation 中に上端帯を公開しない"
            );

            let locked_bottom = NativeEguiOverlay::native_bar_visibility_snapshot(
                true,
                false,
                false,
                false,
                tile_overlay_visible,
                navigation_preview_visible,
            );
            assert_eq!(
                locked_bottom,
                NativeBarVisibilitySnapshot::default(),
                "下部固定も tile/navigation 中は描画・region の双方で抑止する"
            );
        }
    }

    /// 360 度表示中は左右パネルを出さない。見回しドラッグでカーソル / 指が画面端まで
    /// 動くたびにパネルが開き、操作の邪魔になる (2026-08-27 利用者報告)。
    /// ホバーで開く条件が揃っていても、360 が優先して閉じたままにする。
    #[test]
    fn panorama_keeps_both_side_panels_closed_even_while_hovering_the_edges() {
        let right_hovering = NativeRightPanelVisibilityInputs {
            shortcut_help_open: false,
            external_drag_in_progress: false,
            vst3_panel_visible: false,
            metadata_available: true,
            video_speed_popup_open: false,
            hover_preview_active: false,
            tag_picker_open: false,
            pointer_in_hover_rect: true,
            panorama_active: false,
            side_panel_mode: FsSidePanelMode::Hover,
            info_panel_open: crate::ui_helpers::MetadataPanelOpenState::Closed,
        };
        assert!(
            native_right_panel_visible_from_inputs(right_hovering),
            "hovering the right edge opens the panel while the sphere is off"
        );
        assert!(
            !native_right_panel_visible_from_inputs(NativeRightPanelVisibilityInputs {
                panorama_active: true,
                ..right_hovering
            }),
            "a look-around drag must not pull the right panel out"
        );
        // タグピッカーが開いていても 360 が勝つ (ピッカーは通常 hover 条件を迂回する)。
        assert!(
            !native_right_panel_visible_from_inputs(NativeRightPanelVisibilityInputs {
                panorama_active: true,
                tag_picker_open: true,
                ..right_hovering
            }),
            "360 must win over the tag picker's own bypass"
        );

        let jump_hovering = NativeJumpPanelVisibilityInputs {
            shortcut_help_open: false,
            vst3_panel_visible: false,
            video_speed_popup_open: false,
            hover_preview_active: false,
            pointer_in_hover_rect: true,
            panorama_active: false,
            side_panel_mode: FsSidePanelMode::Hover,
            left_panel_open: crate::ui_helpers::MetadataPanelOpenState::Closed,
        };
        assert!(
            native_jump_panel_visible_from_inputs(jump_hovering),
            "hovering the left edge opens the panel while the sphere is off"
        );
        assert!(
            !native_jump_panel_visible_from_inputs(NativeJumpPanelVisibilityInputs {
                panorama_active: true,
                ..jump_hovering
            }),
            "a look-around drag must not pull the left panel out"
        );
        // 明示的に開いた状態で 360 に入っても閉じたままにする。
        assert!(
            !native_jump_panel_visible_from_inputs(NativeJumpPanelVisibilityInputs {
                panorama_active: true,
                pointer_in_hover_rect: false,
                left_panel_open: crate::ui_helpers::MetadataPanelOpenState::ByPointer,
                ..jump_hovering
            }),
            "an explicitly opened panel must also stay closed under 360"
        );
    }

    #[test]
    fn shortcut_help_modal_suppresses_native_edge_panels() {
        let right_base = NativeRightPanelVisibilityInputs {
            shortcut_help_open: false,
            external_drag_in_progress: false,
            vst3_panel_visible: false,
            metadata_available: true,
            video_speed_popup_open: false,
            hover_preview_active: false,
            tag_picker_open: false,
            pointer_in_hover_rect: true,
            panorama_active: false,
            side_panel_mode: FsSidePanelMode::Hover,
            info_panel_open: crate::ui_helpers::MetadataPanelOpenState::Closed,
        };
        assert!(native_right_panel_visible_from_inputs(right_base));
        assert!(!native_right_panel_visible_from_inputs(
            NativeRightPanelVisibilityInputs {
                pointer_in_hover_rect: false,
                panorama_active: false,
                info_panel_open: crate::ui_helpers::MetadataPanelOpenState::ByPointer,
                ..right_base
            }
        ));
        assert!(native_right_panel_visible_from_inputs(
            NativeRightPanelVisibilityInputs {
                pointer_in_hover_rect: false,
                panorama_active: false,
                info_panel_open: crate::ui_helpers::MetadataPanelOpenState::ByTouchHandle,
                ..right_base
            }
        ));
        assert!(!native_right_panel_visible_from_inputs(
            NativeRightPanelVisibilityInputs {
                shortcut_help_open: true,
                ..right_base
            }
        ));
        assert!(native_right_panel_visible_from_inputs(
            NativeRightPanelVisibilityInputs {
                tag_picker_open: true,
                pointer_in_hover_rect: false,
                panorama_active: false,
                ..right_base
            }
        ));
        assert!(!native_right_panel_visible_from_inputs(
            NativeRightPanelVisibilityInputs {
                shortcut_help_open: true,
                tag_picker_open: true,
                pointer_in_hover_rect: false,
                panorama_active: false,
                ..right_base
            }
        ));

        let jump_base = NativeJumpPanelVisibilityInputs {
            shortcut_help_open: false,
            vst3_panel_visible: false,
            video_speed_popup_open: false,
            hover_preview_active: false,
            pointer_in_hover_rect: true,
            panorama_active: false,
            side_panel_mode: FsSidePanelMode::Hover,
            left_panel_open: crate::ui_helpers::MetadataPanelOpenState::Closed,
        };
        assert!(native_jump_panel_visible_from_inputs(jump_base));
        assert!(!native_jump_panel_visible_from_inputs(
            NativeJumpPanelVisibilityInputs {
                pointer_in_hover_rect: false,
                panorama_active: false,
                ..jump_base
            }
        ));
        assert!(native_jump_panel_visible_from_inputs(
            NativeJumpPanelVisibilityInputs {
                pointer_in_hover_rect: false,
                panorama_active: false,
                left_panel_open: crate::ui_helpers::MetadataPanelOpenState::ByTouchHandle,
                ..jump_base
            }
        ));
        assert!(!native_jump_panel_visible_from_inputs(
            NativeJumpPanelVisibilityInputs {
                shortcut_help_open: true,
                ..jump_base
            }
        ));
    }

    #[test]
    fn click_to_show_native_panels_ignore_hover_and_use_explicit_open_state() {
        let right_base = NativeRightPanelVisibilityInputs {
            shortcut_help_open: false,
            external_drag_in_progress: false,
            vst3_panel_visible: false,
            metadata_available: true,
            video_speed_popup_open: false,
            hover_preview_active: false,
            tag_picker_open: false,
            pointer_in_hover_rect: true,
            panorama_active: false,
            side_panel_mode: FsSidePanelMode::ClickToShow,
            info_panel_open: crate::ui_helpers::MetadataPanelOpenState::Closed,
        };
        assert!(!native_right_panel_visible_from_inputs(right_base));
        assert!(native_right_panel_visible_from_inputs(
            NativeRightPanelVisibilityInputs {
                info_panel_open: crate::ui_helpers::MetadataPanelOpenState::ByPointer,
                pointer_in_hover_rect: false,
                panorama_active: false,
                ..right_base
            }
        ));
        assert!(!native_right_panel_visible_from_inputs(
            NativeRightPanelVisibilityInputs {
                metadata_available: false,
                info_panel_open: crate::ui_helpers::MetadataPanelOpenState::ByPointer,
                ..right_base
            }
        ));

        let jump_base = NativeJumpPanelVisibilityInputs {
            shortcut_help_open: false,
            vst3_panel_visible: false,
            video_speed_popup_open: false,
            hover_preview_active: false,
            pointer_in_hover_rect: true,
            panorama_active: false,
            side_panel_mode: FsSidePanelMode::ClickToShow,
            left_panel_open: crate::ui_helpers::MetadataPanelOpenState::Closed,
        };
        assert!(!native_jump_panel_visible_from_inputs(jump_base));
        assert!(native_jump_panel_visible_from_inputs(
            NativeJumpPanelVisibilityInputs {
                left_panel_open: crate::ui_helpers::MetadataPanelOpenState::ByPointer,
                pointer_in_hover_rect: false,
                panorama_active: false,
                ..jump_base
            }
        ));
    }

    #[test]
    fn native_callout_arrow_reverses_when_panel_is_open() {
        assert_eq!(native_panel_callout_arrow_direction(true, false), 1.0);
        assert_eq!(native_panel_callout_arrow_direction(true, true), -1.0);
        assert_eq!(native_panel_callout_arrow_direction(false, false), -1.0);
        assert_eq!(native_panel_callout_arrow_direction(false, true), 1.0);
    }

    #[test]
    fn native_callout_hud_regions_use_only_bars_and_disappear_for_vst() {
        let rects = native_panel_callout_hud_rects(1920.0, 1080.0, true, true, false);
        assert_eq!(
            rects,
            [
                Some(native_panel_callout_bar_rect(1920.0, 1080.0, true)),
                Some(native_panel_callout_bar_rect(1920.0, 1080.0, false)),
            ]
        );
        assert_eq!(
            native_panel_callout_hud_rects(1920.0, 1080.0, true, true, true),
            [None, None]
        );
    }

    #[test]
    fn native_touch_handle_regions_follow_latch() {
        let input = NativeTouchPanelHandleInputs {
            chrome_latched: true,
            blocked: false,
            left_panel_open: false,
            right_panel_open: false,
            right_panel_available: true,
        };
        let [left, right] = native_touch_panel_handle_hud_rects(1200.0, 600.0, input);
        let left = left.unwrap();
        let right = right.unwrap();
        assert_eq!(left.width(), NATIVE_TOUCH_PANEL_HANDLE_WIDTH_PT);
        assert_eq!(right.width(), NATIVE_TOUCH_PANEL_HANDLE_WIDTH_PT);
        assert_eq!(left.height(), right.height());
        assert_eq!(left.top(), right.top());
        assert_eq!(left.bottom(), right.bottom());
        assert_eq!(left.left(), 0.0);
        assert_eq!(right.right(), 1200.0);
        for rect in [left, right] {
            assert!(rect.top() >= native_panel_top());
            assert!(rect.bottom() <= 600.0 - crate::video::native_presenter::HUD_BOTTOM_HEIGHT);
        }
    }

    #[test]
    fn native_touch_handle_regions_hide_unlatched_and_open_sides() {
        let input = NativeTouchPanelHandleInputs {
            chrome_latched: false,
            blocked: false,
            left_panel_open: false,
            right_panel_open: false,
            right_panel_available: true,
        };
        assert_eq!(
            native_touch_panel_handle_hud_rects(1000.0, 800.0, input),
            [None, None]
        );
        let [left, right] = native_touch_panel_handle_hud_rects(
            1000.0,
            800.0,
            NativeTouchPanelHandleInputs {
                chrome_latched: true,
                left_panel_open: true,
                ..input
            },
        );
        assert!(left.is_none());
        assert!(right.is_some());
        let [left, right] = native_touch_panel_handle_hud_rects(
            1000.0,
            800.0,
            NativeTouchPanelHandleInputs {
                chrome_latched: true,
                right_panel_available: false,
                ..input
            },
        );
        assert!(left.is_some());
        assert!(right.is_none());
    }

    #[test]
    fn native_touch_panel_outside_taps_are_consumed_only_for_touch_owner() {
        use crate::ui_helpers::MetadataPanelOpenState::{ByPointer, ByTouchHandle, Closed};
        let seek =
            crate::video::native_touch::NativeVideoTouchCommand::SeekRelative { delta_secs: 5.0 };
        assert!(native_touch_panel_tap_command_dismisses_before_dispatch(
            seek,
            ByTouchHandle,
            ByTouchHandle,
        ));
        assert_eq!(
            native_touch_owned_panel_sides(ByTouchHandle, ByTouchHandle),
            [true, true]
        );
        assert!(native_touch_panel_tap_command_dismisses_before_dispatch(
            crate::video::native_touch::NativeVideoTouchCommand::ToggleChrome,
            Closed,
            ByTouchHandle,
        ));
        assert!(!native_touch_panel_tap_command_dismisses_before_dispatch(
            seek, ByPointer, Closed,
        ));
    }

    #[test]
    fn cursor_move_activity_ignores_zero_delta_moves() {
        // 動画 fullscreen の video→video キーナビ回帰テスト:
        // navigation preview の HUD 全画面化や cursor_polling_tick の synthetic move で
        // 届く「位置不変」の MouseMove は auto-hide 済みカーソルを復帰させない。
        // 直近位置 = (100, 200)、同じ位置への move は hidden の有無に関わらず非活動。
        assert!(!cursor_move_is_activity(Some((100, 200)), (100, 200), true));
        assert!(!cursor_move_is_activity(
            Some((100, 200)),
            (100, 200),
            false
        ));

        // 実際にカーソルが動いた move は、auto-hide 中でも活動 = カーソル復帰。
        assert!(cursor_move_is_activity(Some((100, 200)), (101, 200), true));
        assert!(cursor_move_is_activity(Some((100, 200)), (100, 199), false));

        // 直近位置が不明 (フルスクリーン入場直後など):
        // - 表示中の move は通常どおり活動扱い。
        // - hidden 中の move は region 切替由来の spurious move とみなして抑制する。
        assert!(cursor_move_is_activity(None, (50, 50), false));
        assert!(!cursor_move_is_activity(None, (50, 50), true));
    }

    #[test]
    fn native_overlay_routes_shortcuts_even_when_button_has_focus() {
        crate::keymap::Keymap::empty().install_global_native_video_shortcuts();
        let routing = NativeOverlayInputRouting {
            wants_keyboard_input: true,
            text_input_active: false,
            ..Default::default()
        };

        assert!(routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0x20))));
        assert!(routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0x08))));
        assert!(routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0x0D))));
        assert!(routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0x70))));
        assert!(routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0x75))));
        // F11 (window/fullscreen toggle) も whitelist 経由で App へ流す。
        assert!(routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0x7A))));
        assert!(routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0x49))));
        assert!(routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0x09))));
        assert!(!routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0x41))));
        // マウス進む/戻る (VK_BROWSER_BACK/FORWARD) も overlay が keyboard を欲しがる
        // 状態 (text input 以外) でも fullscreen ショートカットとして App 側へ流す (Codex P2)。
        assert!(routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0xA6))));
        assert!(routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0xA7))));
    }

    #[test]
    fn native_virtual_key_mapping_supports_extended_function_keys() {
        assert_eq!(egui_key_from_virtual_key(0x7C), Some(egui::Key::F13));
        assert_eq!(egui_key_from_virtual_key(0x87), Some(egui::Key::F24));
    }

    #[test]
    fn native_overlay_keeps_shortcuts_while_text_input_is_active() {
        crate::keymap::Keymap::empty().install_global_native_video_shortcuts();
        let routing = NativeOverlayInputRouting {
            wants_keyboard_input: true,
            text_input_active: true,
            ..Default::default()
        };

        assert!(!routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0x20))));
        assert!(!routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0x08))));
        assert!(!routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0x0D))));
        assert!(!routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0x70))));
        assert!(!routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0x75))));
        // text_input_active 中は F11 (window/fullscreen toggle) も UI へ流さない。
        assert!(!routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0x7A))));
        // text_input_active 中はマウス進む/戻るも UI 側へ流さない (= text 編集の邪魔をしない)。
        assert!(!routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0xA6))));
        assert!(!routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0xA7))));
    }

    /// 回帰 (2026-06-19): 動画タグピッカーで Enter 確定すると TextEdit が一瞬フォーカスを
    /// 失い `wants_keyboard_input` が false になる。その隙に Enter が App へ転送されて
    /// fullscreen close = 再生停止していた。`text_input_active` 中は `wants_keyboard_input`
    /// の値に関わらずキーを一切 App へ転送しないこと (focus が無い瞬間も塞ぐ)。
    #[test]
    fn native_overlay_never_forwards_keys_while_text_input_active_without_focus() {
        crate::keymap::Keymap::empty().install_global_native_video_shortcuts();
        let routing = NativeOverlayInputRouting {
            wants_keyboard_input: false,
            text_input_active: true,
            ..Default::default()
        };
        // Enter (0x0D) / Escape (0x1B) / Space (0x20) / F11 (0x7A) いずれも転送しない。
        assert!(!routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0x0D))));
        assert!(!routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0x1B))));
        assert!(!routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0x20))));
        assert!(!routing.should_forward_to_ui(&NativeVideoWindowEvent::KeyDown(key(0x7A))));
    }

    #[test]
    fn text_input_focus_claim_only_when_app_is_foreground_and_focus_is_missing() {
        let target = 0x1000;
        assert!(!should_claim_text_input_focus(false, target, 0, true));
        assert!(!should_claim_text_input_focus(true, 0, 0, true));
        assert!(!should_claim_text_input_focus(true, target, target, true));
        assert!(!should_claim_text_input_focus(true, target, 0, false));
        assert!(should_claim_text_input_focus(true, target, 0, true));
    }

    #[test]
    fn native_video_fullscreen_shortcut_key_allows_alt_keymap_combos() {
        let mut event = key(0x43);
        event.alt = true;
        assert!(native_video_fullscreen_shortcut_key(&event));
    }

    #[test]
    fn seek_status_waits_for_delay_and_holds_after_completion() {
        let now = std::time::Instant::now();
        assert!(!super::seek_status_visible_for_times(
            true,
            Some(now),
            None,
            now + super::SEEK_STATUS_DELAY / 2
        ));
        assert!(super::seek_status_visible_for_times(
            true,
            Some(now),
            None,
            now + super::SEEK_STATUS_DELAY
        ));
        assert!(super::seek_status_visible_for_times(
            false,
            None,
            Some(now),
            now + super::SEEK_STATUS_MIN_VISIBLE / 2
        ));
        assert!(!super::seek_status_visible_for_times(
            false,
            None,
            Some(now),
            now + super::SEEK_STATUS_MIN_VISIBLE
        ));
    }

    #[test]
    fn seek_status_delay_resets_for_new_seek_generation() {
        let now = std::time::Instant::now();
        let second_seek_started = now + std::time::Duration::from_millis(100);
        assert!(!super::seek_status_visible_for_times(
            true,
            Some(second_seek_started),
            None,
            now + super::SEEK_STATUS_DELAY
        ));
    }

    #[test]
    fn native_checkmark_rect_matches_top_right_indicator_position() {
        let rect =
            crate::video::native_presenter::overlay_draw::native_checkmark_rect(1920.0, 28.0);

        assert!((rect.center().x - 1890.0).abs() < f32::EPSILON);
        assert!((rect.center().y - 46.0).abs() < f32::EPSILON);
        assert!((rect.width() - 39.6).abs() < 0.001);
        assert!((rect.height() - 39.6).abs() < 0.001);
    }

    /// 1:1 SAR では従来の isotropic transform と一致 (regression-safe を保証)。
    #[test]
    fn compute_video_visual_transform_sar_1_1_is_isotropic() {
        let (m11, m12, m21, m22, ox, oy) = compute_video_visual_transform(
            1920,
            1080,
            3840,
            2160,
            1,
            1,
            VideoOrientation::IDENTITY,
            false,
        );
        assert_eq!((m12, m21), (0.0, 0.0));
        assert!(
            (m11 - m22).abs() < 1e-6,
            "M11 ({m11}) should equal M22 ({m22})"
        );
        assert!((m11 - 2.0).abs() < 1e-6, "1920->3840 should be 2x");
        assert!(ox.abs() < 1e-3, "centered horizontally");
        assert!(oy.abs() < 1e-3, "centered vertically");
    }

    #[test]
    fn compute_video_visual_transform_display_resolution_surface_is_position_only() {
        // The shader has already resolved the frame to the 1600x900 physical
        // display rectangle above the locked top bar. DComp only positions that
        // surface inside the same reserved target rect used by surface policy.
        let (m11, m12, m21, m22, ox, oy) = compute_video_visual_transform(
            1600,
            900,
            1600,
            900 + HUD_TOP_HEIGHT as u32,
            1,
            1,
            VideoOrientation::IDENTITY,
            VideoVisualLayout {
                compact: false,
                pixels_per_point: 1.0,
                top_bar_locked: true,
                bottom_lock: VideoBottomLock::None,
                seek_strip_visible: false,
                fixed_bar_gap_px: 0,
            },
        );
        assert!((m11 - 1.0).abs() < f32::EPSILON);
        assert!((m22 - 1.0).abs() < f32::EPSILON);
        assert_eq!(m12, 0.0);
        assert_eq!(m21, 0.0);
        assert_eq!(ox, 0.0);
        assert_eq!(oy, HUD_TOP_HEIGHT);
    }

    #[test]
    fn clearing_panorama_pose_returns_resolve_ownership_to_normal_resampling() {
        let pose = PanoPose::new(
            0.0,
            0.0,
            crate::panorama::FOV_DEFAULT,
            crate::panorama::PanoProjection::Perspective,
        );
        let mut state = Some((pose, PanoUvTransform::IDENTITY));
        assert_eq!(
            VideoSurfaceContent::resolved(state.is_some()),
            VideoSurfaceContent::PanoramaResolved
        );
        state = None;
        assert_eq!(
            VideoSurfaceContent::resolved(state.is_some()),
            VideoSurfaceContent::DisplayResolved
        );
    }

    #[test]
    fn panorama_surface_fills_the_visual_target_without_letterboxing() {
        let (m11, m12, m21, m22, ox, oy) = compute_video_visual_transform_for_surface(
            1920,
            960,
            1920,
            1080,
            1,
            1,
            VideoOrientation::IDENTITY,
            VideoVisualLayout::from(false),
            true,
        );
        assert_eq!((m12, m21), (0.0, 0.0));
        assert!((m11 - 1.0).abs() < f32::EPSILON);
        assert!((m22 - 1.125).abs() < 1.0e-6);
        assert_eq!((ox, oy), (0.0, 0.0));
    }

    /// orientation=0 は回転対応前の SAR transform と完全に同じ値を返す。
    #[test]
    fn compute_video_visual_transform_rotation_zero_preserves_legacy_values() {
        assert_eq!(
            compute_video_visual_transform(
                640,
                480,
                1920,
                1080,
                1,
                1,
                VideoOrientation::IDENTITY,
                false,
            ),
            (2.25, 0.0, 0.0, 2.25, 240.0, 0.0)
        );
    }

    #[test]
    fn compute_video_visual_transform_quarter_turns_transpose_fit() {
        let turn_90 = compute_video_visual_transform(
            1280,
            720,
            1920,
            1080,
            1,
            1,
            VideoOrientation::new(90, false),
            false,
        );
        assert_eq!(turn_90, (0.0, 0.84375, -0.84375, 0.0, 1263.75, 0.0));

        let turn_180 = compute_video_visual_transform(
            1280,
            720,
            1920,
            1080,
            1,
            1,
            VideoOrientation::new(180, false),
            false,
        );
        assert_eq!(turn_180, (-1.5, 0.0, 0.0, -1.5, 1920.0, 1080.0));

        let turn_270 = compute_video_visual_transform(
            1280,
            720,
            1920,
            1080,
            1,
            1,
            VideoOrientation::new(270, false),
            false,
        );
        assert_eq!(turn_270, (0.0, -0.84375, 0.84375, 0.0, 656.25, 1080.0));
    }

    #[test]
    fn compute_video_visual_transform_preserves_reflection() {
        let reflected = compute_video_visual_transform(
            640,
            480,
            1920,
            1080,
            1,
            1,
            VideoOrientation::new(180, true),
            false,
        );
        assert_eq!(reflected, (-2.25, 0.0, 0.0, 2.25, 1680.0, 0.0));
    }

    /// SAR 97/80 (= 1.2125、本件動画) で 720x480 を 1920x1080 に letterbox fit。
    /// 表示寸法 = 720*1.2125 × 480 = 873x480、aspect 1.819:1 なので window 16:9 に
    /// 横いっぱい (1920 幅) 入るはず。M11/M22 比は SAR と一致する。
    #[test]
    fn compute_video_visual_transform_anamorphic_97_80() {
        let (m11, _, _, m22, ox, oy) = compute_video_visual_transform(
            720,
            480,
            1920,
            1080,
            97,
            80,
            VideoOrientation::IDENTITY,
            false,
        );
        let ratio = m11 / m22;
        assert!(
            (ratio - 1.2125).abs() < 1e-4,
            "M11/M22 should equal SAR 1.2125 (got {ratio})"
        );
        // display_w = 720 * 1.2125 = 873, display_h = 480, win 1920x1080
        // scale = min(1920/873, 1080/480) = min(2.199, 2.25) = 2.199
        // → 横いっぱい、縦に余白
        let expected_scale = 1920.0_f32 / (720.0 * 97.0 / 80.0);
        assert!((m22 - expected_scale).abs() < 1e-3);
        assert!(ox.abs() < 0.5, "should fit horizontally edge-to-edge");
        assert!(oy > 0.0, "should letterbox vertically");
    }

    /// SAR 0/0 (未指定)・0/1・1/0 はすべて 1:1 として扱う (= max(1) で正規化)。
    #[test]
    fn compute_video_visual_transform_zero_sar_normalizes_to_one() {
        let baseline = compute_video_visual_transform(
            720,
            480,
            1920,
            1080,
            1,
            1,
            VideoOrientation::IDENTITY,
            false,
        );
        assert_eq!(
            compute_video_visual_transform(
                720,
                480,
                1920,
                1080,
                0,
                0,
                VideoOrientation::IDENTITY,
                false,
            ),
            baseline,
            "0/0 must equal 1/1"
        );
        assert_eq!(
            compute_video_visual_transform(
                720,
                480,
                1920,
                1080,
                0,
                1,
                VideoOrientation::IDENTITY,
                false,
            ),
            baseline,
            "0/1 must equal 1/1"
        );
        assert_eq!(
            compute_video_visual_transform(
                720,
                480,
                1920,
                1080,
                1,
                0,
                VideoOrientation::IDENTITY,
                false,
            ),
            baseline,
            "1/0 must equal 1/1"
        );
    }

    /// SAR < 1 (縦アナモフィック、稀) でも letterbox が成立する。
    #[test]
    fn compute_video_visual_transform_vertical_anamorphic() {
        let (m11, _, _, m22, _ox, _oy) = compute_video_visual_transform(
            960,
            540,
            1920,
            1080,
            1,
            2,
            VideoOrientation::IDENTITY,
            false,
        );
        // SAR=0.5 → display_w=480, display_h=540, ratio M11/M22 = 0.5
        let ratio = m11 / m22;
        assert!((ratio - 0.5).abs() < 1e-4);
    }

    #[test]
    fn fixed_video_bars_reserve_target_rect_for_all_combinations_and_gap_bounds() {
        let target = |top_bar_locked, bottom_lock, seek_strip_visible, fixed_bar_gap_px| {
            compute_video_visual_target_rect(
                1920,
                1080,
                VideoVisualLayout {
                    compact: false,
                    pixels_per_point: 1.0,
                    top_bar_locked,
                    bottom_lock,
                    seek_strip_visible,
                    fixed_bar_gap_px,
                },
            )
        };

        assert_eq!(
            target(false, VideoBottomLock::None, false, 0),
            VideoVisualTargetRect {
                x: 0.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
            }
        );
        assert_eq!(
            target(true, VideoBottomLock::None, false, 0),
            VideoVisualTargetRect {
                x: 0.0,
                y: HUD_TOP_HEIGHT,
                width: 1920.0,
                height: 1080.0 - HUD_TOP_HEIGHT,
            }
        );
        assert_eq!(
            target(false, VideoBottomLock::BarOnly, false, 0),
            VideoVisualTargetRect {
                x: 0.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0 - HUD_BOTTOM_HEIGHT,
            }
        );
        assert_eq!(
            target(true, VideoBottomLock::BarOnly, false, 0),
            VideoVisualTargetRect {
                x: 0.0,
                y: HUD_TOP_HEIGHT,
                width: 1920.0,
                height: 1080.0 - HUD_TOP_HEIGHT - HUD_BOTTOM_HEIGHT,
            }
        );

        let max_gap = crate::settings::FULLSCREEN_FIXED_BAR_GAP_MAX_PX;
        assert_eq!(
            target(true, VideoBottomLock::BarOnly, false, max_gap),
            VideoVisualTargetRect {
                x: 0.0,
                y: HUD_TOP_HEIGHT + max_gap as f32,
                width: 1920.0,
                height: 1080.0 - HUD_TOP_HEIGHT - HUD_BOTTOM_HEIGHT - max_gap as f32 * 2.0,
            }
        );
        assert_eq!(
            target(true, VideoBottomLock::BarOnly, false, max_gap + 900),
            target(true, VideoBottomLock::BarOnly, false, max_gap),
            "余白は静止画と同じ上限で clamp する"
        );
    }

    #[test]
    fn video_bottom_lock_reserves_the_strip_only_for_the_visible_locked_strip() {
        let expected = [
            (VideoBottomLock::None, false, 0.0),
            (VideoBottomLock::None, true, 0.0),
            (VideoBottomLock::BarOnly, false, HUD_BOTTOM_HEIGHT),
            (VideoBottomLock::BarOnly, true, HUD_BOTTOM_HEIGHT),
            (VideoBottomLock::BarAndStrip, false, HUD_BOTTOM_HEIGHT),
            (
                VideoBottomLock::BarAndStrip,
                true,
                HUD_BOTTOM_HEIGHT + SEEK_STRIP_HEIGHT,
            ),
        ];
        for (bottom_lock, seek_strip_visible, bottom_reserved) in expected {
            assert_eq!(
                compute_video_visual_target_rect(
                    1920,
                    1080,
                    VideoVisualLayout {
                        compact: false,
                        pixels_per_point: 1.0,
                        top_bar_locked: false,
                        bottom_lock,
                        seek_strip_visible,
                        fixed_bar_gap_px: 0,
                    },
                ),
                VideoVisualTargetRect {
                    x: 0.0,
                    y: 0.0,
                    width: 1920.0,
                    height: 1080.0 - bottom_reserved,
                }
            );
        }
    }

    #[test]
    fn compact_video_uses_the_upper_right_quarter_of_the_reserved_bar_target() {
        let target = compute_video_visual_target_rect(
            1920,
            1080,
            VideoVisualLayout {
                compact: true,
                pixels_per_point: 1.0,
                top_bar_locked: true,
                bottom_lock: VideoBottomLock::BarOnly,
                seek_strip_visible: false,
                fixed_bar_gap_px: 10,
            },
        );
        assert_eq!(
            target,
            VideoVisualTargetRect {
                x: 960.0,
                y: HUD_TOP_HEIGHT + 10.0,
                width: 960.0,
                height: (1080.0 - HUD_TOP_HEIGHT - HUD_BOTTOM_HEIGHT - 20.0) * 0.5,
            }
        );
    }

    #[test]
    fn native_bar_lock_buttons_are_inside_the_same_snapshot_regions_as_their_bars() {
        let width = 1280.0;
        let height = 720.0;
        let top_locked_visible =
            NativeEguiOverlay::native_hud_top_visible_from_hover(None, false, false, false, true);
        let seek_locked_visible = NativeEguiOverlay::native_hud_bottom_visible_from_hover(
            None, height, false, false, true,
        );
        let snapshot = NativeEguiOverlay::native_bar_visibility_snapshot(
            seek_locked_visible,
            top_locked_visible,
            false,
            false,
            false,
            false,
        );
        assert_eq!(
            snapshot,
            NativeBarVisibilitySnapshot {
                top_bar_visible: true,
                bottom_hud_visible: true,
            }
        );
        let top_region =
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(width, HUD_TOP_HEIGHT));
        let bottom_region = egui::Rect::from_min_max(
            egui::pos2(0.0, height - HUD_BOTTOM_HEIGHT),
            egui::pos2(width, height),
        );
        let top_lock = native_top_bar_lock_button_rect(width);
        let seek_lock = native_seek_bar_lock_button_rect(width, height);
        let seek_strip_selector = native_seek_strip_selector_button_rect(width, height);

        assert!(top_region.contains(top_lock.min) && top_region.contains(top_lock.max));
        assert!(bottom_region.contains(seek_lock.min) && bottom_region.contains(seek_lock.max));
        assert!(
            bottom_region.contains(seek_strip_selector.min)
                && bottom_region.contains(seek_strip_selector.max)
        );
        assert!(seek_strip_selector.max.x < seek_lock.min.x);
    }

    /// Compact mode (VST3 panel 表示時の 1/4 領域) でも SAR 補正が正しく適用される。
    #[test]
    fn compute_video_visual_transform_compact_mode_respects_sar() {
        let (m11_normal, _, _, m22_normal, _, _) = compute_video_visual_transform(
            720,
            480,
            1920,
            1080,
            97,
            80,
            VideoOrientation::IDENTITY,
            false,
        );
        let (m11_compact, _, _, m22_compact, ox, oy) = compute_video_visual_transform(
            720,
            480,
            1920,
            1080,
            97,
            80,
            VideoOrientation::IDENTITY,
            true,
        );
        // compact = 1/4 領域なので scale も半分。M11/M22 比は SAR で同じ。
        let ratio_normal = m11_normal / m22_normal;
        let ratio_compact = m11_compact / m22_compact;
        assert!((ratio_normal - ratio_compact).abs() < 1e-5);
        assert!(m11_compact < m11_normal, "compact should fit smaller area");
        // compact 領域は右上 (target_x = win_w/2 = 960、target_y = 0)
        assert!(ox >= 960.0, "compact target starts at right half");
        assert!(oy >= 0.0);
    }

    /// 不正な (極めて小さい / 0) ウィンドウやサーフェイスでも panic しない。
    #[test]
    fn compute_video_visual_transform_handles_zero_dims() {
        // surface 0/0 → max(1) 正規化、計算が NaN/inf にならず数値で返ること
        let (m11, m12, m21, m22, ox, oy) = compute_video_visual_transform(
            0,
            0,
            1920,
            1080,
            1,
            1,
            VideoOrientation::IDENTITY,
            false,
        );
        assert!(m12.is_finite() && m21.is_finite());
        assert!(m11.is_finite() && m22.is_finite() && ox.is_finite() && oy.is_finite());
        let (m11, m12, m21, m22, ox, oy) =
            compute_video_visual_transform(720, 480, 0, 0, 1, 1, VideoOrientation::IDENTITY, false);
        assert!(m12.is_finite() && m21.is_finite());
        assert!(m11.is_finite() && m22.is_finite() && ox.is_finite() && oy.is_finite());
    }

    #[test]
    fn copy_cpu_rgba_to_swapchain_bgra_swaps_red_and_blue() {
        let src = [
            0x10, 0x20, 0x30, 0x40, //
            0xaa, 0xbb, 0xcc, 0xdd,
        ];
        let mut dst = Vec::new();

        copy_cpu_rgba_to_swapchain_bgra(&src, &mut dst, 2, 1).unwrap();

        assert_eq!(
            dst,
            [
                0x30, 0x20, 0x10, 0x40, //
                0xcc, 0xbb, 0xaa, 0xdd,
            ]
        );
    }

    #[test]
    fn copy_cpu_rgba_to_swapchain_bgra_rejects_short_input() {
        let mut dst = Vec::new();

        let err = copy_cpu_rgba_to_swapchain_bgra(&[0, 1, 2], &mut dst, 1, 1).unwrap_err();

        assert!(err.contains("too small"));
    }

    #[test]
    fn sample_cpu_rgba_pixel_returns_bgra_at_center() {
        let src = [
            1, 2, 3, 4, //
            5, 6, 7, 8, //
            9, 10, 11, 12, //
            13, 14, 15, 16,
        ];

        let sample = sample_cpu_rgba_pixel(&src, 2, 2, 87).unwrap();

        assert_eq!(sample.x, 1);
        assert_eq!(sample.y, 1);
        assert_eq!((sample.b, sample.g, sample.r, sample.a), (15, 14, 13, 16));
    }

    #[test]
    fn compare_pixel_probe_detects_channel_mismatch() {
        let expected = NativePixelSample {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            format: 87,
            b: 10,
            g: 20,
            r: 30,
            a: 255,
        };
        let actual = NativePixelSample {
            r: 10,
            b: 30,
            ..expected
        };

        let err = compare_pixel_probe("cpu_upload", expected, actual).unwrap_err();

        assert!(err.contains("mismatch"));
    }

    #[test]
    fn metadata_clean_text_preserves_description_line_breaks() {
        let text = " line one  with   spaces\\n\\nline two\r\n  line three  ";

        assert_eq!(
            metadata_clean_text(text),
            "line one with spaces\n\nline two\nline three"
        );
    }
}
