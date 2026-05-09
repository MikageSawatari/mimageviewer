//! フルスクリーン表示のレンダリング。
//!
//! `App::update()` から呼ばれる `render_fullscreen_viewport()` を実装する。
//! 元は `update()` 内にインラインで書かれていた ~460 行を独立メソッドに切り出したもの。

use eframe::egui;

use crate::app::App;
use crate::fs_animation::FsCacheEntry;
use crate::grid_item::{GridItem, ThumbnailState};
use crate::pdf_loader::PdfPageContentType;
use crate::settings::SpreadMode;
use crate::ui_helpers::open_external_player;

mod draw_icons;
use self::draw_icons::*;

// ── 定数 ────────────────────────────────────────────────────────────────

/// メタデータパネルの最大幅
const METADATA_PANEL_WIDTH: f32 = 380.0;
/// ホバー時トップバーの高さ
const TOP_BAR_HEIGHT: f32 = 44.0;
/// ホイール感度（raw_scroll_delta の除数）
const WHEEL_SENSITIVITY: f32 = 30.0;

/// 中ボタンドラッグズーム: 縦 N px で倍率 2 倍/半分になる感度 (v0.8.1)。
/// 100 px で 2 倍 (= 上へ 200 px で 4 倍)。ホイール 1 ノッチ ≈ 10% と比べて粗めだが、
/// 縦フル (1080 px) ストロークすれば 2^10 ≈ 1000 倍まで届くので十分。
const MIDDLE_DRAG_UNIT_PX: f32 = 100.0;
/// 中ボタン押下から「ドラッグ開始」とみなす最小移動量。
/// この距離以下ならズームは触らない (クリックのみとの区別 / 暴発防止)。
const MIDDLE_DRAG_THRESHOLD_PX: f32 = 4.0;
/// ズーム倍率の下限
const ZOOM_MIN: f32 = 0.1;
/// ズーム倍率の上限
const ZOOM_MAX: f32 = 50.0;
/// ズームが 1.0 とみなせるしきい値
const ZOOM_NEAR_ONE: f32 = 1.001;
/// 回転・パンがゼロとみなせるしきい値
const TRANSFORM_EPSILON: f32 = 0.001;
/// パンがゼロとみなせるしきい値（length_sq）
const PAN_EPSILON_SQ: f32 = 0.25;
/// バー内ボタンのサイズ
const BAR_BUTTON_SIZE: f32 = 32.0;
/// バー内ボタンの上下マージン
const BAR_BUTTON_MARGIN: f32 = 6.0;
/// バー内ボタン間の隙間
const BAR_BUTTON_GAP: f32 = 4.0;
/// チェックマーク円の半径
const CHECKMARK_RADIUS: f32 = 18.0;
/// 透過画像背景の市松 1 タイルサイズ (px)
const CHECKER_TILE_PX: f32 = 16.0;

#[cfg(windows)]
struct NativeFocusClaim {
    foreground_hwnd: usize,
    post_foreground_hwnd: usize,
    target_hwnd: usize,
    set_foreground_ok: bool,
    attach_thread_input_ok: bool,
    set_active_ok: bool,
    set_focus_ok: bool,
}

#[cfg(windows)]
struct NativeFocusTarget {
    foreground_hwnd: usize,
    target_hwnd: usize,
}

#[cfg(windows)]
fn native_window_under_cursor_focus_target() -> NativeFocusTarget {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::{
        GA_ROOT, GetAncestor, GetCursorPos, GetForegroundWindow, WindowFromPoint,
    };

    unsafe {
        let mut pt = POINT::default();
        if GetCursorPos(&mut pt).is_err() {
            return NativeFocusTarget {
                foreground_hwnd: GetForegroundWindow().0 as usize,
                target_hwnd: 0,
            };
        }
        let hovered = WindowFromPoint(pt);
        let foreground = GetForegroundWindow();
        if hovered.0.is_null() {
            return NativeFocusTarget {
                foreground_hwnd: foreground.0 as usize,
                target_hwnd: 0,
            };
        }
        let root = GetAncestor(hovered, GA_ROOT);
        let target = if root.0.is_null() { hovered } else { root };
        NativeFocusTarget {
            foreground_hwnd: foreground.0 as usize,
            target_hwnd: target.0 as usize,
        }
    }
}

#[cfg(windows)]
fn claim_native_window_focus(target_hwnd: usize) -> NativeFocusClaim {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::Input::KeyboardAndMouse::{SetActiveWindow, SetFocus};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow,
    };

    unsafe {
        let target = HWND(target_hwnd as *mut std::ffi::c_void);
        let foreground = GetForegroundWindow();
        let this_tid = GetCurrentThreadId();
        let foreground_tid = if !foreground.0.is_null() {
            GetWindowThreadProcessId(foreground, None)
        } else {
            0
        };
        let attached = foreground_tid != 0
            && foreground_tid != this_tid
            && AttachThreadInput(this_tid, foreground_tid, true).as_bool();
        let set_foreground_ok = SetForegroundWindow(target).as_bool();
        let set_active_ok = SetActiveWindow(target).is_ok();
        let set_focus_ok = SetFocus(Some(target)).is_ok();
        let post_foreground = GetForegroundWindow();
        if attached {
            let _ = AttachThreadInput(this_tid, foreground_tid, false);
        }
        NativeFocusClaim {
            foreground_hwnd: foreground.0 as usize,
            post_foreground_hwnd: post_foreground.0 as usize,
            target_hwnd: target.0 as usize,
            set_foreground_ok,
            attach_thread_input_ok: attached,
            set_active_ok,
            set_focus_ok,
        }
    }
}

#[cfg(not(windows))]
struct NativeFocusClaim {
    foreground_hwnd: usize,
    post_foreground_hwnd: usize,
    target_hwnd: usize,
    set_foreground_ok: bool,
    attach_thread_input_ok: bool,
    set_active_ok: bool,
    set_focus_ok: bool,
}

#[cfg(not(windows))]
struct NativeFocusTarget {
    foreground_hwnd: usize,
    target_hwnd: usize,
}

#[cfg(not(windows))]
fn native_window_under_cursor_focus_target() -> NativeFocusTarget {
    NativeFocusTarget {
        foreground_hwnd: 0,
        target_hwnd: 0,
    }
}

#[cfg(not(windows))]
fn claim_native_window_focus(target_hwnd: usize) -> NativeFocusClaim {
    NativeFocusClaim {
        foreground_hwnd: 0,
        post_foreground_hwnd: 0,
        target_hwnd,
        set_foreground_ok: false,
        attach_thread_input_ok: false,
        set_active_ok: false,
        set_focus_ok: false,
    }
}

#[cfg(windows)]
fn current_foreground_hwnd() -> usize {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    unsafe { GetForegroundWindow().0 as usize }
}

#[cfg(not(windows))]
fn current_foreground_hwnd() -> usize {
    0
}

#[cfg(windows)]
fn original_preview_shortcut_held(_ctx: &egui::Context) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_RCONTROL};

    unsafe { GetAsyncKeyState(VK_RCONTROL.0 as i32) < 0 }
}

#[cfg(not(windows))]
fn original_preview_shortcut_held(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.key_down(egui::Key::Num0))
}

fn is_fullscreen_shortcut_probe_key(key: egui::Key) -> bool {
    matches!(
        key,
        egui::Key::ArrowLeft
            | egui::Key::ArrowRight
            | egui::Key::ArrowUp
            | egui::Key::ArrowDown
            | egui::Key::W
            | egui::Key::Enter
            | egui::Key::Escape
            | egui::Key::Space
            | egui::Key::S
            | egui::Key::M
            | egui::Key::L
            | egui::Key::J
            | egui::Key::K
            | egui::Key::B
            | egui::Key::P
            | egui::Key::I
            | egui::Key::Z
            | egui::Key::R
    )
}

fn fullscreen_shortcut_event_summary(ctx: &egui::Context) -> Option<String> {
    let parts = ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|event| {
                if let egui::Event::Key {
                    key,
                    pressed,
                    repeat,
                    modifiers,
                    ..
                } = event
                    && is_fullscreen_shortcut_probe_key(*key)
                {
                    Some(format!(
                        "{:?}:{}{}:{:?}",
                        key,
                        if *pressed { "down" } else { "up" },
                        if *repeat { ":repeat" } else { "" },
                        modifiers
                    ))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
    });
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(","))
    }
}

#[cfg(windows)]
fn native_video_vk_from_egui_key(key: egui::Key) -> Option<u32> {
    Some(match key {
        egui::Key::Enter => 0x0D,
        egui::Key::Escape => 0x1B,
        egui::Key::Space => 0x20,
        egui::Key::End => 0x23,
        egui::Key::Home => 0x24,
        egui::Key::ArrowLeft => 0x25,
        egui::Key::ArrowUp => 0x26,
        egui::Key::ArrowRight => 0x27,
        egui::Key::ArrowDown => 0x28,
        egui::Key::B => 0x42,
        egui::Key::J => 0x4A,
        egui::Key::K => 0x4B,
        egui::Key::L => 0x4C,
        egui::Key::M => 0x4D,
        egui::Key::P => 0x50,
        egui::Key::S => 0x53,
        egui::Key::W => 0x57,
        _ => return None,
    })
}

#[cfg(windows)]
fn native_video_key_events_from_ctx(
    ctx: &egui::Context,
) -> Vec<crate::video::native_window::NativeVideoKeyEvent> {
    ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|event| {
                if let egui::Event::Key {
                    key,
                    pressed: true,
                    repeat,
                    modifiers,
                    ..
                } = event
                {
                    native_video_vk_from_egui_key(*key).map(|virtual_key| {
                        crate::video::native_window::NativeVideoKeyEvent {
                            virtual_key,
                            shift: modifiers.shift,
                            ctrl: modifiers.ctrl,
                            alt: modifiers.alt,
                            repeat: *repeat,
                        }
                    })
                } else {
                    None
                }
            })
            .collect()
    })
}

/// 補正ショートカット (U/P/N) のスコープ。どの層を書き換えるかを表す。
/// 解決 (`App::resolve_adjust_scope`) と書き込み (`App::write_params_for_scope`) は
/// App 側にメソッドとして実装され、ここには enum 定義とラベルだけ置く。
#[derive(Clone, Copy)]
pub(crate) enum AdjustScope {
    PageOverride,
    FavoriteDefault(uuid::Uuid),
    Global,
}

impl AdjustScope {
    /// トースト表示用ラベル。
    #[inline]
    pub(crate) fn label(self) -> &'static str {
        match self {
            AdjustScope::PageOverride => "個別",
            AdjustScope::FavoriteDefault(_) => "お気に入り",
            AdjustScope::Global => "標準",
        }
    }
}

/// 見開き描画時に書き出されるページ矩形レイアウト。
/// ルーペ描画がカーソル位置からどちらのページかを判定し、UV サンプリングに使う。
#[derive(Clone, Copy)]
pub struct FsSpreadLayout {
    pub left_idx: usize,
    pub left_rect: egui::Rect,
    pub right_idx: usize,
    pub right_rect: egui::Rect,
}

/// 透過背景の描画スタイル。B キーで 3 モードを循環する。
///
/// フルスクリーンのビューポート背景は `ui_fullscreen.rs` で `Color32::BLACK` に
/// ハードコードされており、テーマ設定 (Light/Dark/System) に関係なく常に黒。
/// そのため B キー循環は「黒 (= ビューポート既定) → 白 → 市松」のテーマ非依存
/// 3 モードとした。以前はテーマの反対色を計算していたが、Light テーマ時に
/// `反対色 = 黒 = ビューポート既定` となり 2 モード連続で視覚変化なしになるバグが
/// あったため撤去 (v0.7.0 フィードバック)。
pub(crate) enum FsBgStyle<'a> {
    /// 塗らない (0 = ビューポート既定 / 常に黒地)
    Default,
    /// 単色で塗りつぶす (1 = 白)
    Solid(egui::Color32),
    /// 市松パターン (2)。テクスチャは Wrap=Repeat で作成済みであること。
    Checker(&'a egui::TextureHandle),
}

/// B キーで選択されたモードから描画スタイルを構築する。
///
/// - mode = 0: `Default` (塗らない — ビューポート既定の黒が透けて見える)
/// - mode = 1: `Solid(WHITE)`
/// - mode = 2: `Checker` (中間グレー市松)
pub(crate) fn transparent_bg_style<'a>(
    mode: u8,
    checker: Option<&'a egui::TextureHandle>,
) -> FsBgStyle<'a> {
    match mode {
        1 => FsBgStyle::Solid(egui::Color32::WHITE),
        2 => match checker {
            Some(t) => FsBgStyle::Checker(t),
            None => FsBgStyle::Default,
        },
        _ => FsBgStyle::Default,
    }
}

/// B キー循環で使うモードのトーストラベル。
pub(crate) fn transparent_bg_toast(mode: u8) -> &'static str {
    match mode {
        1 => "[背景: 白]",
        2 => "[背景: 市松]",
        _ => "[背景: 黒]",
    }
}

/// `rect` 内に透過背景を描画する。画像テクスチャを描く**直前**に呼ぶこと。
pub(crate) fn paint_transparent_bg(
    painter: &egui::Painter,
    rect: egui::Rect,
    style: &FsBgStyle<'_>,
) {
    match style {
        FsBgStyle::Default => {}
        FsBgStyle::Solid(color) => {
            painter.rect_filled(rect, 0.0, *color);
        }
        FsBgStyle::Checker(tex) => {
            // テクスチャは Wrap=Repeat で 16×16 の市松。
            // rect 全域をカバーするよう UV を rect_size / tile_px で指定する。
            let uv_max = egui::pos2(
                rect.width() / CHECKER_TILE_PX,
                rect.height() / CHECKER_TILE_PX,
            );
            let uv_rect = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), uv_max);
            painter.image(tex.id(), rect, uv_rect, egui::Color32::WHITE);
        }
    }
}
/// チェックマーク円のマージン（画面端からの距離）
const CHECKMARK_MARGIN: f32 = 16.0;
/// 見開き表示の区切り線の幅 (px)
const SPREAD_DIVIDER_WIDTH: f32 = 2.0;
/// フィードバックトースト表示時間（秒）
const FEEDBACK_TOAST_DURATION: f32 = 1.2;
/// 境界ヒント（最初/最後の画像に達した案内）の表示時間（秒）
const BOUNDARY_HINT_DURATION: f32 = 2.5;
/// 画像フォルダが見つからない旨のヒント表示時間（秒）。メッセージが長く
/// ユーザーがフルスクリーンを維持するか Esc で抜けるか判断する時間が要るため、
/// 境界ヒントより長めに取る。
const NO_IMAGE_FOLDER_HINT_DURATION: f32 = 4.0;

/// J/K でマーカー間ジャンプするときの「現在位置とみなす許容幅」(秒)。
/// 現在位置とほぼ同じマーカーをスキップして次のマーカーへ進めるための余裕。
const NAV_MARKER_EPSILON: f64 = 0.5;

/// 動画再生中、最後のユーザー操作からこの時間が経過したら HUD のフェードを開始する (秒)。
const VIDEO_HUD_IDLE_BEFORE_FADE: f32 = 2.0;
/// 動画 HUD のフェードアウトに掛ける時間 (秒)。`IDLE_BEFORE_FADE` を超えた直後から
/// この時間で 1.0 → 0.0 まで滑らかに減衰する。
const VIDEO_HUD_FADE_DURATION: f32 = 0.3;

/// フルスクリーンで上部ホバーバーが表示される画面上端からの距離 (ピクセル)。
/// `draw_fs_hover_bar` の hover 判定と、`fs_ui_is_clean` のクリーン判定で共有する。
const TOP_BAR_HOVER_Y: f32 = 60.0;
// CURSOR_HIDE_IDLE_SECS は `crate::video::native_presenter::CURSOR_HIDE_IDLE_SECS`
// に集約している (eframe 経路と D3D11 native 経路の両方で同じ閾値を使うため)。

/// 動画のチャプター・ブックマーク・ピンを 1 本の Vec に集約するための種別タグ。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum NavMarkerKind {
    Chapter,
    Bookmark,
    Pin,
}

/// 動画再生時のジャンプ可能マーカー (チャプター開始 / ブックマーク / ピン)。
/// シークバー描画と J/K ジャンプの両方から使う。
#[derive(Clone, Debug)]
pub(crate) struct NavMarker {
    pub pts: f64,
    pub kind: NavMarkerKind,
    pub title: Option<String>,
}

/// シークバーのマーカー縦線の色 (kind 別)。チャプター=水色 / ブックマーク=黄 / ピン=緑。
pub(crate) fn nav_marker_color(kind: NavMarkerKind) -> egui::Color32 {
    match kind {
        NavMarkerKind::Chapter => egui::Color32::from_rgb(120, 200, 255),
        NavMarkerKind::Bookmark => egui::Color32::from_rgb(255, 220, 80),
        NavMarkerKind::Pin => egui::Color32::from_rgb(100, 230, 130),
    }
}

/// フルスクリーン中央のヒントオーバーレイ。
#[derive(Copy, Clone)]
pub(crate) enum FsBoundaryHint {
    /// 最初/最後の画像に到達 (at_end: true=末尾, false=先頭)。
    Edge {
        at_end: bool,
        at: std::time::Instant,
    },
    /// Ctrl+↑↓ で画像のある次 (forward=true) / 前 (forward=false) のフォルダが
    /// skip_limit 以内に見つからなかった。
    NoImageFolder {
        forward: bool,
        at: std::time::Instant,
    },
    /// Ctrl+G 絞り込みビューで、これ以上進める検索結果が無い
    /// (forward=true: 末端, forward=false: 先頭)。
    SearchEnd {
        forward: bool,
        at: std::time::Instant,
    },
}

impl FsBoundaryHint {
    pub(crate) fn started_at(&self) -> std::time::Instant {
        match self {
            FsBoundaryHint::Edge { at, .. }
            | FsBoundaryHint::NoImageFolder { at, .. }
            | FsBoundaryHint::SearchEnd { at, .. } => *at,
        }
    }
}

// ── 見開きペアリング ──────────────────────────────────────────────────────

/// 見開き表示のペア解決結果。
#[derive(Copy, Clone)]
pub(crate) enum SpreadPair {
    /// 単独表示（1ページ表示 / 横長画像 / 表紙 / 末尾余り）
    Single,
    /// 見開き表示: left=画面左に表示するidx, right=画面右に表示するidx
    Double { left: usize, right: usize },
}

impl App {
    /// 分析モードを解除し、関連する状態をリセットする。
    pub(crate) fn reset_analysis_mode(&mut self) {
        let restore_idx = self.fullscreen_idx;
        self.analysis_mode = false;
        // post-filter バイパスを解除 (消しゴムモード中ならそちらが保持する)
        if self.post_filter_bypassed && !self.erase_mode {
            self.post_filter_bypassed = false;
            if let Some(idx) = restore_idx {
                self.clear_adjustment_caches(idx);
            }
        }
        self.analysis_hover_color = None;
        self.analysis_pinned_color = None;
        self.analysis_grayscale = false;
        self.analysis_mosaic_grid = false;
        self.analysis_filter_mag = 0;
        self.analysis_guide_drag = None;
    }

    /// 分析モード ON 時に post-filter を一時バイパスする。Z キーハンドラから呼ぶ。
    pub(crate) fn enter_analysis_mode_bypass(&mut self) {
        if !self.post_filter_bypassed {
            self.post_filter_bypassed = true;
            if let Some(idx) = self.fullscreen_idx {
                self.clear_adjustment_caches(idx);
            }
        }
    }

    /// `idx` に対する「現在表示できる最良のテクスチャ」を Arc::clone で取り出す。
    /// 優先順: 補正済みキャッシュ → AI 処理済み → fs_cache (Static / Animated 現フレーム)
    /// → サムネ (`include_thumb=true` のときのみ)。
    ///
    /// `prepare_fullscreen_state` の高解像度 tex 解決と `current_fs_tex_for_holdover`
    /// が同じチェーンを 2 回書いていた重複を集約する。Render パスは AI 設定 OFF 時に
    /// AI キャッシュを使わない gate を別途持っているので、フル一致ではなく一部
    /// (補正・fs_cache) を共通化する位置付け。Holdover は AI gate を気にせず常にこの
    /// チェーンを走らせる (短時間の lock 中は古い AI 表示でも黒画面より遥かにマシ)。
    pub(crate) fn resolve_fs_display_tex(
        &self,
        idx: usize,
        include_thumb: bool,
    ) -> Option<egui::TextureHandle> {
        if let Some(FsCacheEntry::Static { tex, .. }) = self.adjustment_cache.get(&idx) {
            return Some(tex.clone());
        }
        let bg = self.effective_upscale_bg_mode();
        if let Some(FsCacheEntry::Static { tex, .. }) = self.ai_upscale_cache.get(&(idx, bg)) {
            return Some(tex.clone());
        }
        match self.fs_cache.get(&idx) {
            Some(FsCacheEntry::Static { tex, .. }) => return Some(tex.clone()),
            Some(FsCacheEntry::Animated {
                frames,
                current_frame,
                ..
            }) => {
                if let Some((h, _)) = frames.get(*current_frame) {
                    return Some(h.clone());
                }
            }
            Some(FsCacheEntry::Video { .. }) => {
                // 動画は native presenter が独立 HWND に描画するため、
                // ここから取り出せる egui TextureHandle はない。サムネイルへ
                // フォールバックする。
            }
            _ => {}
        }
        if include_thumb {
            if let Some(crate::grid_item::ThumbnailState::Loaded { tex, .. }) =
                self.thumbnails.get(idx)
            {
                return Some(tex.clone());
            }
        }
        None
    }

    /// 右 Ctrl ホールド中だけ、mIV 側の派生表示 (補正 / AI / 消しゴム補完) を
    /// 迂回して元画像テクスチャを選ぶ。
    ///
    /// フォーカスは `ctx.input(|i| i.viewport().focused)` ではなく
    /// `self.fs_prev_focused` で確認する: この関数の呼び出し元
    /// `prepare_fullscreen_state` は `show_viewport_immediate` の **外側** で
    /// 呼ばれているので、ここでの ctx はメインビューポートのもの = フルスクリーンが
    /// OS フォーカスを持っている間は常に `Some(false)` を返してしまう。
    /// `fs_prev_focused` はフルスクリーン viewport closure 内で毎フレーム更新される
    /// ので、フルスクリーン側の前フレーム focus 状態を反映する。`GetAsyncKeyState`
    /// 単体だと動画/アニメ駆動の repaint 中に他アプリの右Ctrl を拾うリスクがあるので、
    /// 必ずフォーカス gate と AND させる。
    fn original_preview_active(&self, ctx: &egui::Context, idx: usize) -> bool {
        if !self.fs_prev_focused {
            return false;
        }
        if self.any_modal_dialog_open_for_fullscreen_keys() {
            return false;
        }
        if !original_preview_shortcut_held(ctx) {
            return false;
        }
        matches!(
            self.items.get(idx),
            Some(GridItem::Image(_))
                | Some(GridItem::ZipImage { .. })
                | Some(GridItem::PdfPage { .. })
        )
    }

    fn resolve_original_preview_tex(
        &mut self,
        ctx: &egui::Context,
        idx: usize,
    ) -> Option<egui::TextureHandle> {
        if let Some(base) = self.erase_base_cache.get(&idx) {
            let needs_upload = self
                .original_preview_tex_cache
                .get(&idx)
                .map(|tex| tex.size() != base.size)
                .unwrap_or(true);
            if needs_upload {
                let tex = ctx.load_texture(
                    format!("fs_original_preview_{idx}"),
                    base.as_ref().clone(),
                    egui::TextureOptions::LINEAR,
                );
                self.original_preview_tex_cache.insert(idx, tex);
            }
            return self.original_preview_tex_cache.get(&idx).cloned();
        }

        match self.fs_cache.get(&idx) {
            Some(FsCacheEntry::Static { tex, .. }) => Some(tex.clone()),
            Some(FsCacheEntry::Animated {
                frames,
                current_frame,
                ..
            }) => frames.get(*current_frame).map(|(h, _)| h.clone()),
            _ => None,
        }
    }

    /// Ctrl+↑↓ ナビ発火直前に `fs_holdover_tex` を仕込むためのヘルパ。
    /// `resolve_fs_display_tex` を `include_thumb=true` で呼んで「最良の表示物」を取る。
    pub(crate) fn current_fs_tex_for_holdover(&self, fs_idx: usize) -> Option<egui::TextureHandle> {
        self.resolve_fs_display_tex(fs_idx, true)
    }

    /// `fs_nav_locked_gen.is_some()` の薄いラッパー。
    /// 入力ハンドラ・描画パスから「現在 nav ロック中か」を簡潔に問い合わせるため。
    pub(crate) fn fs_nav_is_locked(&self) -> bool {
        self.fs_nav_locked_gen.is_some()
    }

    /// nav ロックと holdover を強制解除する。`poll_fs_nav_lock` の通常解除条件
    /// (items_generation 進行 + 新ページの tex 用意) に到達しないケース
    /// (DFS が境界に当たって path=None / 画像フォルダに着地できず !hit_image_folder /
    /// ユーザーが Esc でフルスクリーンを抜けて DFS をキャンセル) で明示的に呼ぶ。
    /// これをやらないと `fs_nav_locked_gen` が永続化して以降の Ctrl+↑↓ がすべて
    /// 無視される (Codex P1)。
    pub(crate) fn release_fs_nav_lock(&mut self) {
        self.fs_nav_locked_gen = None;
        self.fs_holdover_tex = None;
    }

    /// Ctrl+↑↓ ナビ発火直前に `fs_holdover_tex` を仕込み、`items_generation` を
    /// ロック取得時点で記録する。ナビによる items 入れ替えで fs_cache が drop されても、
    /// ロック解除まで holdover Arc を Render パスから参照することで画面が真っ白に
    /// なるのを防ぐ。`items_generation` のスナップショットは `poll_fs_nav_lock` の
    /// 「items が入れ替わる前にロックを解除しない」判定に使う。
    pub(crate) fn capture_fs_nav_holdover(&mut self, fs_idx: usize) {
        self.fs_holdover_tex = self.current_fs_tex_for_holdover(fs_idx);
        self.fs_nav_locked_gen = Some(self.items_generation);
    }

    /// 毎フレーム呼び出され、ナビロックの解除条件を満たしたら lock を解除する。
    /// 解除条件: ① items が入れ替わって `items_generation` が進んだ
    /// (= `install_new_items` で新フォルダの items が導入された) かつ
    /// ② 現フルスクリーン idx に対して thumbnails が `Loaded` か
    /// fs_cache に Static / Animated エントリが入った状態。
    /// 「items が進む前」(= 旧ページがロード済み判定で誤って解除される) のを
    /// items_generation チェックで防ぐ。
    pub(crate) fn poll_fs_nav_lock(&mut self) {
        let Some(locked_gen) = self.fs_nav_locked_gen else {
            return;
        };
        // items_generation が進んでいない = まだ items 入れ替えが起きていないので
        // 旧 fs_idx のテクスチャが残っているのは当然。ここで解除すると holdover が
        // 失われて「ファイル名のみ表示」のフラッシュが出るので保留する。
        if self.items_generation <= locked_gen {
            return;
        }
        let Some(idx) = self.fullscreen_idx else {
            // ユーザーが Esc 等でフルスクリーンを抜けた場合のみここに来る
            // (apply_folder_nav_result 内の close_fullscreen は同フレーム内で
            //  open_fullscreen に続くので fs_idx は Some に戻る)。
            self.fs_nav_locked_gen = None;
            self.fs_holdover_tex = None;
            return;
        };
        let has_full = matches!(
            self.fs_cache.get(&idx),
            Some(FsCacheEntry::Static { .. })
                | Some(FsCacheEntry::Animated { .. })
                | Some(FsCacheEntry::Video { .. })
        );
        let has_thumb = matches!(
            self.thumbnails.get(idx),
            Some(crate::grid_item::ThumbnailState::Loaded { .. })
        );
        if has_full || has_thumb {
            self.fs_nav_locked_gen = None;
            self.fs_holdover_tex = None;
        }
    }

    /// フルスクリーン通常モードのズーム/パンが有効なら返す。
    /// 閾値以下なら None (描画側で無変換パスに流れるよう明示するため)。
    pub(crate) fn fs_zoom_pan(&self) -> Option<(f32, egui::Vec2)> {
        if self.fs_zoom > ZOOM_NEAR_ONE || self.fs_pan.length_sq() > PAN_EPSILON_SQ {
            Some((self.fs_zoom, self.fs_pan))
        } else {
            None
        }
    }

    /// 中ボタンドラッグズームを処理する。
    ///
    /// ホイール押し込み + 上下ドラッグで fs_zoom / analysis_zoom を連続的に変える。
    /// 分析モード中は analysis_zoom/pan、それ以外は fs_zoom/pan を書き換える
    /// (どちらに書き込むかはドラッグ開始時点の `analysis_mode` で決まる)。
    /// 戻り値は「このフレームで中ボタンがアクティブに使われていた」か。true なら
    /// 呼び出し側はこの後の左クリック/右クリックの解釈をスキップしてよい。
    fn handle_middle_drag_zoom(&mut self, ctx: &egui::Context, full_rect: egui::Rect) -> bool {
        let (is_down, is_pressed, is_released, current_pos) = ctx.input(|i| {
            (
                i.pointer.button_down(egui::PointerButton::Middle),
                i.pointer.button_pressed(egui::PointerButton::Middle),
                i.pointer.button_released(egui::PointerButton::Middle),
                i.pointer.interact_pos(),
            )
        });

        // 押下開始フレーム: 現在のズーム/パン/ピボットをスナップショット。
        // 同フレームで既にドラッグ中なら無視 (重複開始を防ぐ)。
        if is_pressed && self.fs_middle_zoom_drag.is_none() {
            if let Some(pos) = current_pos {
                // 分析モードでは画像エリアが右パネル分左にずれるので中心が違う
                let rect_center = if self.analysis_mode {
                    analysis_image_rect(full_rect).center()
                } else {
                    full_rect.center()
                };
                let (start_zoom, start_pan) = if self.analysis_mode {
                    (self.analysis_zoom, self.analysis_pan)
                } else {
                    (self.fs_zoom, self.fs_pan)
                };
                self.fs_middle_zoom_drag = Some(MiddleZoomDrag {
                    pivot: pos,
                    start_zoom,
                    start_pan,
                    rect_center,
                    is_analysis: self.analysis_mode,
                });
            }
        }

        // ドラッグ中: 押しっぱなしの間、毎フレーム pivot からの差分で新しいズームを計算。
        if is_down {
            if let (Some(drag), Some(pos)) = (self.fs_middle_zoom_drag.clone(), current_pos) {
                let dy = pos.y - drag.pivot.y;
                if dy.abs() < MIDDLE_DRAG_THRESHOLD_PX {
                    // しきい値以下: ズーム変更なし (クリック暴発防止)
                    return true;
                }
                // 上方向 (dy < 0) で拡大、下方向で縮小
                let factor = 2.0_f32.powf(-dy / MIDDLE_DRAG_UNIT_PX);
                let new_zoom = (drag.start_zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
                // pivot 位置が画面上で動かないように pan を補正。ホイールズームと違い
                // 差分累積ではなく毎回 start 値基準で計算することで、dy が元に戻れば
                // pan も完全に元へ戻る (累積誤差が発生しない)。
                let new_pan = zoom_preserve_pivot(
                    drag.pivot,
                    drag.rect_center,
                    drag.start_pan,
                    drag.start_zoom,
                    new_zoom,
                );
                // 書き戻し先はドラッグ開始時の is_analysis で固定
                // (途中でモードが切り替わっても書き先がブレないように)
                if drag.is_analysis {
                    self.analysis_zoom = new_zoom;
                    self.analysis_pan = new_pan;
                } else {
                    self.fs_zoom = new_zoom;
                    self.fs_pan = new_pan;
                }
                return true;
            }
        }

        // リリース: PDF のときだけ新倍率で再レンダリング要求 (ドラッグ中は発行しない)。
        if is_released {
            if let Some(drag) = self.fs_middle_zoom_drag.take() {
                let final_zoom = if drag.is_analysis {
                    self.analysis_zoom
                } else {
                    self.fs_zoom
                };
                if (final_zoom - drag.start_zoom).abs() > f32::EPSILON {
                    self.maybe_rerender_pdf(final_zoom);
                }
                return true;
            }
        }

        false
    }

    /// ホイールによるマウス位置固定ズームを適用する。ズームが変化したら true を返す。
    fn apply_wheel_zoom(
        zoom: &mut f32,
        pan: &mut egui::Vec2,
        wheel_y: f32,
        mouse: Option<egui::Pos2>,
        rect_center: egui::Pos2,
    ) -> bool {
        let factor = 1.1_f32.powf(wheel_y / WHEEL_SENSITIVITY);
        let old_zoom = *zoom;
        *zoom = (old_zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
        if let Some(mouse) = mouse {
            *pan = zoom_preserve_pivot(mouse, rect_center, *pan, old_zoom, *zoom);
        }
        *zoom != old_zoom
    }

    /// 現在のフルスクリーン画像が PDF ページなら、指定ズームで再レンダリングを要求する。
    fn maybe_rerender_pdf(&mut self, zoom: f32) {
        if let Some(idx) = self.fullscreen_idx {
            if matches!(self.items.get(idx), Some(GridItem::PdfPage { .. })) {
                self.request_pdf_rerender(idx, zoom);
            }
        }
    }
}

/// ピボット点 (`mouse`) が画面上で動かないように、zoom 変化に合わせて pan を補正した
/// 新しい pan を返す。`apply_wheel_zoom` と `handle_middle_drag_zoom` が共用する。
///
/// 式: new_pan = base_pan + (mouse - (rect_center + base_pan)) * (1 - new_zoom / base_zoom)
fn zoom_preserve_pivot(
    mouse: egui::Pos2,
    rect_center: egui::Pos2,
    base_pan: egui::Vec2,
    base_zoom: f32,
    new_zoom: f32,
) -> egui::Vec2 {
    let center = rect_center + base_pan;
    let cx = mouse.x - center.x;
    let cy = mouse.y - center.y;
    let ratio = new_zoom / base_zoom;
    egui::vec2(
        base_pan.x + cx * (1.0 - ratio),
        base_pan.y + cy * (1.0 - ratio),
    )
}

/// 中ボタンドラッグズームのスナップショット状態 (v0.8.1)。
/// ドラッグ開始フレームで固定し、以降はここからの差分でズームを計算する。
#[derive(Clone)]
pub(crate) struct MiddleZoomDrag {
    /// ドラッグ開始時のカーソル位置 (このピボットが画面上で動かないように pan を補正)
    pub pivot: egui::Pos2,
    /// 開始時の zoom (fs_zoom または analysis_zoom のいずれか)
    pub start_zoom: f32,
    /// 開始時の pan
    pub start_pan: egui::Vec2,
    /// ピボット計算に使う画像エリアの中心 (通常モードは full_rect.center()、
    /// 分析モードは analysis_image_rect.center())
    pub rect_center: egui::Pos2,
    /// ドラッグ開始時に分析モードだったか (途中でモード切替されても書き戻し先を固定)
    pub is_analysis: bool,
}

/// 分析モード時の画像表示領域（パネル分を右側に確保した残り）を返す。
fn analysis_image_rect(full_rect: egui::Rect) -> egui::Rect {
    let panel_w = 360.0_f32.clamp(full_rect.width() * 0.20, full_rect.width() * 0.35);
    egui::Rect::from_min_max(
        full_rect.min,
        egui::pos2(full_rect.max.x - panel_w, full_rect.max.y),
    )
}

/// VST3 コンパクト表示モード時の動画表示領域 (= 右上 1/4 = 幅・高さ各 1/2)。
/// 残った左下 3/4 は黒背景のままなのでプラグイン GUI ウィンドウを置きやすい。
fn vst3_compact_image_rect(full_rect: egui::Rect) -> egui::Rect {
    let half_w = full_rect.width() * 0.5;
    let half_h = full_rect.height() * 0.5;
    egui::Rect::from_min_max(
        egui::pos2(full_rect.max.x - half_w, full_rect.min.y),
        egui::pos2(full_rect.max.x, full_rect.min.y + half_h),
    )
}

/// ナビゲーション可能アイテムのインデックスリストを作成する。
/// `adjacent_navigable_idx` と同じフィルタ条件。
fn build_nav_indices(items: &[GridItem], visible_indices: &[usize]) -> Vec<usize> {
    visible_indices
        .iter()
        .copied()
        .filter(|&i| {
            matches!(
                items.get(i),
                Some(GridItem::Image(_))
                    | Some(GridItem::Video(_))
                    | Some(GridItem::ZipImage { .. })
                    | Some(GridItem::ZipSeparator { .. })
                    | Some(GridItem::PdfPage { .. })
            )
        })
        .collect()
}

/// 指定インデックスの画像が横長（幅>高さ）かを判定する。
/// テクスチャサイズが不明な場合は false（縦長として扱う）。
fn is_landscape(
    idx: usize,
    fs_cache: &std::collections::HashMap<usize, FsCacheEntry>,
    thumbnails: &[ThumbnailState],
) -> bool {
    // フルサイズキャッシュから判定
    if let Some(entry) = fs_cache.get(&idx) {
        match entry {
            FsCacheEntry::Static { tex, .. } => {
                let s = tex.size_vec2();
                return s.x > s.y;
            }
            FsCacheEntry::Animated { frames, .. } => {
                if let Some((tex, _)) = frames.first() {
                    let s = tex.size_vec2();
                    return s.x > s.y;
                }
            }
            FsCacheEntry::Video { player, .. } => {
                if let Some(info) = player.info() {
                    return info.width > info.height;
                }
            }
            FsCacheEntry::Failed => {}
        }
    }
    // サムネイルから判定
    if let Some(ThumbnailState::Loaded { tex, .. }) = thumbnails.get(idx) {
        let s = tex.size_vec2();
        return s.x > s.y;
    }
    false
}

// ── フルスクリーン状態の中間構造体 ──────────────────────────────────────

/// フルスクリーン描画 1 フレーム分の事前計算済み状態。
struct FsFrameState {
    is_video: bool,
    separator_text: Option<String>,
    original_preview_active: bool,
    tex: Option<egui::TextureHandle>,
    thumb_tex: Option<egui::TextureHandle>,
    /// 上部ホバーバー左側に表示するパス文字列。
    /// 通常画像は `<folder>\<filename>`、ZIP 内画像は `<archive-path> > <entry>`、
    /// PDF ページは `<pdf-path> > Page N` の形で事前に整形して格納する。
    /// 変換済みアーカイブ閲覧中は `archive_source_override` を用いて元の
    /// 7z/LZH のパスを表示する (キャッシュ ZIP のパスは見せない)。
    location_display: String,
    image_dims: Option<(u32, u32)>,
    image_file_size: Option<u64>,
    /// 原寸が GPU テクスチャ上限 (MAX_TEXTURE_DIM=8192) を超えていて、
    /// 表示は縮小版を使っているとき true。ホバーバーに⚠マーカーを出すのに使う。
    image_downscaled: bool,
    is_loading: bool,
    /// 起動時 VST3 チェーンロードが終わるまで動画開始を待っている状態。
    vst3_waiting_for_video: bool,
    fs_load_failed: bool,
    /// PDF ページのコンテンツ種別 (非 PDF なら None)
    pdf_content_type: Option<PdfPageContentType>,
}

/// フルスクリーンのキー入力結果。
pub(crate) struct FsKeyAction {
    pub(crate) close: bool,
    pub(crate) nav_delta: i32,
    pub(crate) ctrl_nav: Option<i32>,
    /// Home/End などの絶対ジャンプ先 item index
    pub(crate) jump_to: Option<usize>,
}

impl App {
    fn fullscreen_viewport_id(&self) -> egui::ViewportId {
        egui::ViewportId::from_hash_of(("fullscreen_viewer", self.fs_viewport_generation))
    }

    /// フルスクリーンビューポートを描画し、終了後のナビゲーション処理も行う。
    /// フルスクリーン表示中でなければ何もしない。
    /// フルスクリーンが非アクティブでもビューポートを非表示で維持する。
    /// アプリ起動直後から呼ばれ、初回のフルスクリーン表示時のちらつきを防ぐ。
    pub(crate) fn keep_fullscreen_viewport_alive(&mut self, ctx: &egui::Context) {
        if self.fullscreen_idx.is_some() {
            return; // アクティブなときは render_fullscreen_viewport が担当
        }
        let fs_id = self.fullscreen_viewport_id();

        // Ctrl+↑↓ で PDF フォルダを挟む遷移中は fullscreen_idx が None のまま
        // PDF enumerate 完了を待つ。この間ビューポートを隠すとその下のグリッドが
        // 見えてちらつくので維持しつつ、ナビロックの holdover (= 直前ページの
        // テクスチャ) があればそれを表示して「黒画面で待たされる」体感を緩和する。
        if self.fs_viewport_shown && self.fs_nav_after_pdf_enumerate.is_some() {
            let fs_builder = self.build_fullscreen_viewport_builder();
            let mut cancel = false;
            // holdover を中央フィットで描画する用のテクスチャ参照をクロージャ前に外出し。
            let holdover = self.fs_holdover_tex.clone();
            ctx.show_viewport_immediate(fs_id, fs_builder, |ctx, _class| {
                // 列挙が重い / ワーカー異常停止などで待ちが長くなったときに
                // ユーザーが黒画面に閉じ込められないよう、Esc とウィンドウ
                // クローズ要求を受け付けて保留中の遷移をキャンセルする。
                if ctx.input(|i| i.viewport().close_requested())
                    || ctx.input(|i| i.key_pressed(egui::Key::Escape))
                {
                    cancel = true;
                }
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(egui::Color32::BLACK))
                    .show(ctx, |ui| {
                        if let Some(handle) = holdover.as_ref() {
                            // 中央 contain フィット (= はみ出さないアスペクト維持)。
                            let avail = ui.available_size();
                            let tex_size = handle.size_vec2();
                            if tex_size.x > 0.0
                                && tex_size.y > 0.0
                                && avail.x > 0.0
                                && avail.y > 0.0
                            {
                                let scale = (avail.x / tex_size.x).min(avail.y / tex_size.y);
                                let w = tex_size.x * scale;
                                let h = tex_size.y * scale;
                                let img_rect = egui::Rect::from_center_size(
                                    ui.max_rect().center(),
                                    egui::vec2(w, h),
                                );
                                ui.painter().image(
                                    handle.id(),
                                    img_rect,
                                    egui::Rect::from_min_max(
                                        egui::pos2(0.0, 0.0),
                                        egui::pos2(1.0, 1.0),
                                    ),
                                    egui::Color32::WHITE,
                                );
                            }
                        }
                    });
            });
            if cancel {
                // 保留中の「列挙後にフルスクリーン復帰」意図を破棄。
                // poll_pdf_enumerate 完了時のフルスクリーン再オープンが抑止され、
                // 次フレーム以降はこの関数の非アクティブ経路でビューポートが
                // 隠される (グリッドへ戻る)。
                self.fs_nav_after_pdf_enumerate = None;
                ctx.request_repaint();
            }
            return;
        }

        // 非表示でもフルスクリーンサイズを維持する。
        // 1x1 → フルサイズへのリサイズが Visible(true) と同時に発生すると
        // OS のウィンドウマネージャが中間状態を描画してちらつく。
        let fs_builder = self.build_fullscreen_viewport_builder().with_visible(false);
        ctx.show_viewport_immediate(fs_id, fs_builder, |_ctx, _class| {});
        // ViewportBuilder::with_visible(false) は「initial」可視性しか制御しないため、
        // 一度表示済みのビューポートを隠すには明示的に Visible(false) を送る必要がある。
        // 送信直前に DWM トランジションを無効化して Win11 のフェードアウトを抑止する。
        if self.fs_viewport_shown {
            crate::dwm_transitions::disable_transitions_for_thread_windows();
            ctx.send_viewport_cmd_to(fs_id, egui::ViewportCommand::Visible(false));
            self.fs_viewport_shown = false;
            if self.fs_viewport_recreate_after_hide {
                self.fs_viewport_generation = self.fs_viewport_generation.wrapping_add(1);
                self.fs_viewport_recreate_after_hide = false;
            }
        }
    }

    pub(crate) fn render_fullscreen_viewport(&mut self, ctx: &egui::Context) {
        let Some(fs_idx) = self.fullscreen_idx else {
            return;
        };

        // ── pending の PDF 再レンダリング結果を取り込む ──
        // show_viewport_immediate 内では &mut self が使えるので、
        // メインの update() を待たずにここで直接 poll する。
        self.poll_prefetch(ctx);

        // ── 状態の事前計算 ──
        #[cfg(windows)]
        if self.native_video_backdrop_target_for_fs(fs_idx) {
            self.show_native_video_black_backdrop(ctx, fs_idx);
            return;
        }
        #[cfg(windows)]
        if self.native_video_presenter_pending_for_fs(fs_idx) {
            self.show_native_video_black_backdrop(ctx, fs_idx);
            return;
        }
        #[cfg(windows)]
        if self.native_video_presenter_hwnd_for_fs(fs_idx).is_some() {
            self.show_native_video_black_backdrop(ctx, fs_idx);
            return;
        }

        self.advance_animation(ctx, fs_idx);
        // 見開きペアを 1 回だけ解決し、以降のフレーム処理で再利用する
        // (resolve_spread_pair は get_nav_indices 内で Vec<usize> をクローンするため、
        //  毎フレーム 3〜4 回呼ばれるのを避ける)
        let spread_pair = self.resolve_spread_pair(fs_idx);
        let is_spread_double = matches!(spread_pair, SpreadPair::Double { .. });
        // 見開きパートナーの事前読み込み + アニメーション進行
        if let SpreadPair::Double { left, right } = spread_pair {
            let partner = if left == fs_idx { right } else { left };
            self.advance_animation(ctx, partner);
            if !self.fs_cache.contains_key(&partner) && !self.fs_pending.contains_key(&partner) {
                self.start_fs_load(partner);
            }
        }
        let state = self.prepare_fullscreen_state(ctx, fs_idx);

        let mut close_fs = false;
        let mut nav_delta: i32 = 0;
        let mut ctrl_nav: Option<i32> = None;
        let mut jump_to: Option<usize> = None;
        // 境界ヒント即時消去のため、フレーム先頭の状態を捕捉する。
        // handle_fs_navigation 実行後に、ヒントが同じ start_time のまま残って
        // いれば「このフレームで再設定されていない」= 打ち切ってよい、と判定する。
        let hint_start_before = self.fs_boundary_hint.map(|h| h.started_at());
        let mut had_user_input_in_frame = false;
        let prev_foreground_hwnd = self.fs_prev_foreground_hwnd;

        // ── ビューポート構築 ──
        let fs_builder = self.build_fullscreen_viewport_builder();
        let need_show = !self.fs_viewport_shown;
        let fs_viewport_t0 = std::time::Instant::now();
        let mut fs_setup_ms = 0.0_f64;
        let mut fs_input_ms = 0.0_f64;
        let mut fs_media_ms = 0.0_f64;
        let mut fs_overlay_ms = 0.0_f64;
        let mut fs_hud_ms = 0.0_f64;
        let mut fs_panels_ms = 0.0_f64;
        let mut fs_hover_bar_ms = 0.0_f64;
        let mut fs_central_ms = 0.0_f64;
        let mut fs_vst_manager_ms = 0.0_f64;
        let mut fs_closure_ms = 0.0_f64;
        let fs_state_is_video = state.is_video;
        // 動画は native presenter が独立 HWND に描画するので、egui 側 viewport は
        // 黒 backdrop のみ。ここで GPU 経路かどうかを区別する必要は無い。
        let fs_state_gpu_video = false;

        ctx.show_viewport_immediate(
            self.fullscreen_viewport_id(),
            fs_builder,
            |ctx, _class| {
                let closure_t0 = std::time::Instant::now();
                let setup_t0 = std::time::Instant::now();
                // フルスクリーンビューポート内のイベントで IME 状態を更新する
                // (メインビューポートとは別のイベントキューなのでここで呼ぶ必要がある)
                self.update_ime_state(ctx);
                if need_show {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }

                // 他アプリからフォーカスが戻ってきた瞬間を記録。
                // この直後のクリックはナビ目的ではなく「フォーカスを戻すだけのクリック」
                // とみなし、アプリ側の処理を抑制する（ページ送り・パン開始など）。
                let focused_now = ctx.input(|i| i.viewport().focused).unwrap_or(true);
                if focused_now && !self.fs_prev_focused {
                    self.fs_focus_regained_at = Some(std::time::Instant::now());
                }
                self.fs_prev_focused = focused_now;

                // カーソル自動非表示用のアクティビティ検出。マウス移動 / クリック /
                // ホイール / キー入力のいずれかがあれば「活動中」とみなしタイマをリセット。
                // `pointer.velocity()` ではなく `delta()` を使う (velocity は静止後も
                // 慣性を残し、3 秒タイマがいつまでも進まない誤動作になるため)。
                // `open_fullscreen` で Some 初期化されるが、想定外の入場経路で None の
                // まま到達した場合も safety net として今フレームで起動する。
                if self.cursor_last_activity.is_none() {
                    self.cursor_last_activity = Some(std::time::Instant::now());
                }
                let cursor_active = ctx.input(|i| {
                    i.pointer.delta() != egui::Vec2::ZERO
                        || i.pointer.any_pressed()
                        || i.pointer.any_click()
                        || i.smooth_scroll_delta != egui::Vec2::ZERO
                        || i.events.iter().any(|e| {
                            matches!(
                                e,
                                egui::Event::Key { pressed: true, .. } | egui::Event::Text(_)
                            )
                        })
                });
                if cursor_active {
                    self.cursor_last_activity = Some(std::time::Instant::now());
                    self.cursor_hidden = false;
                }

                // event consume される前に捕捉 (handle_fs_key_input が矢印等を
                // 消費するとイベントが見えなくなるため)。マウス移動は操作と見なさない。
                if self.fs_boundary_hint.is_some() {
                    had_user_input_in_frame = ctx.input(|i| {
                        i.events.iter().any(|e| matches!(
                            e,
                            egui::Event::Key { pressed: true, .. }
                                | egui::Event::PointerButton { pressed: true, .. }
                                | egui::Event::MouseWheel { .. }
                        ))
                    });
                }

                if ctx.input(|i| i.viewport().close_requested()) {
                    close_fs = true;
                }
                fs_setup_ms = setup_t0.elapsed().as_secs_f64() * 1000.0;

                let central_t0 = std::time::Instant::now();
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(egui::Color32::BLACK))
                    .show(ctx, |ui| {
                        let full_rect = ui.max_rect();
                        let input_t0 = std::time::Instant::now();

                        // ── キー入力 ──
                        if let Some(keys) = fullscreen_shortcut_event_summary(ctx) {
                            let focused = ctx.input(|i| i.viewport().focused).unwrap_or(true);
                            crate::logger::log(format!(
                                "[fs-key] source=fullscreen focused={} foreground=0x{:x} keys={}",
                                focused,
                                current_foreground_hwnd(),
                                keys
                            ));
                        }
                        let key_action = self.handle_fs_key_input(ctx, fs_idx, is_spread_double);
                        if key_action.close { close_fs = true; }
                        nav_delta = key_action.nav_delta;
                        ctrl_nav = key_action.ctrl_nav;
                        jump_to = key_action.jump_to;
                        // perf: キー起因のナビはここで input_seq を進める
                        if nav_delta != 0 {
                            self.bump_input_seq("fs_key", Some(&format!("delta={nav_delta}")));
                        } else if ctrl_nav.is_some() {
                            self.bump_input_seq("fs_ctrl_nav", None);
                        } else if key_action.close {
                            self.bump_input_seq("fs_close_key", None);
                        }

                        // ── ホイール & クリック ──
                        let (wheel_nav, click_close) = self.handle_fs_wheel_and_click(
                            ui,
                            ctx,
                            full_rect,
                            &state,
                            is_spread_double,
                            prev_foreground_hwnd,
                        );
                        if wheel_nav != 0 { nav_delta = wheel_nav; }
                        if click_close { close_fs = true; }
                        // perf: ホイール/クリック起因のナビ
                        if wheel_nav != 0 {
                            self.bump_input_seq("fs_wheel", Some(&format!("delta={wheel_nav}")));
                        } else if click_close {
                            self.bump_input_seq("fs_close_click", None);
                        }
                        // ホイール/キーで nav_delta が確定済みなら、
                        // ホバーバーのボタンホバーで上書きされないよう保護
                        let nav_locked = nav_delta != 0;
                        fs_input_ms = input_t0.elapsed().as_secs_f64() * 1000.0;

                        // ── 分析/補正モード: 見開き中は無効 ──
                        // 分析モードは画像エリアを左側に制限する（右パネルと重ならないよう）。
                        // 補正モードは左パネルを画像の上にオーバーレイする（画像位置は移動しない）。
                        let analysis_active = self.analysis_mode && !is_spread_double;
                        // 補正パネルは見開き Double でも使えるようにする (左右独立補正 + コピー)。
                        // 編集対象 (画面上の左/右) は `adjust_spread_target` で切替。
                        let adjustment_active = self.adjustment_mode;
                        // VST3 動画コンパクト表示モード: 動画のときだけ右上 1/4 に縮小し、
                        // 残った左下 3/4 をプラグイン GUI 用に空ける。動画でない (画像/PDF)
                        // ときは無視する (= プラグインで分析するのは動画なので)。
                        let vst3_compact_active =
                            cfg!(windows) && state.is_video && self.settings.vst3_enabled
                                && self.settings.vst3_gui_visible
                                && self.settings.vst3_video_compact;
                        let image_rect = if analysis_active {
                            analysis_image_rect(full_rect)
                        } else if vst3_compact_active {
                            vst3_compact_image_rect(full_rect)
                        } else {
                            full_rect
                        };

                        // ── 画像 / 動画 / セパレータ描画 ──
                        let media_t0 = std::time::Instant::now();
                        if let Some(sep) = state.separator_text.as_ref() {
                            Self::draw_fs_separator(ui, image_rect, sep);
                        } else {
                            match spread_pair {
                                SpreadPair::Single => {
                                    let fs_rotation = self.get_rotation(fs_idx);
                                    let zp = if analysis_active {
                                        Some((self.analysis_zoom, self.analysis_pan))
                                    } else {
                                        self.fs_zoom_pan()
                                    };
                                    let free_rot = if analysis_active { 0.0 } else { self.fs_free_rotation };
                                    // 前フレームと異なる (idx, テクスチャ) の最初の描画で paint を emit。
                                    // seq はエントリ自身の `load_seq` を使う (self.input_seq だと
                                    // paint 時点で別操作に更新されていて load→ready→paint の相関が崩れる)。
                                    if crate::perf::is_enabled()
                                        && let Some(tex) = state.tex.as_ref()
                                    {
                                        let cur_id = tex.id();
                                        let prev = self.fs_painted_last;
                                        let is_new = !matches!(
                                            prev,
                                            Some((prev_idx, prev_id, _)) if prev_idx == fs_idx && prev_id == cur_id
                                        );
                                        if is_new {
                                            let key = self.perf_item_key(fs_idx);
                                            let entry_seq = self
                                                .fs_cache
                                                .get(&fs_idx)
                                                .map(|e| e.load_seq())
                                                .unwrap_or(0);
                                            crate::perf::event(
                                                "fs",
                                                "paint",
                                                key.as_deref(),
                                                entry_seq,
                                                &[("idx", serde_json::Value::from(fs_idx))],
                                            );
                                            self.fs_painted_last = Some((fs_idx, cur_id, entry_seq));
                                        }
                                    }
                                    let bg_style = self.fs_bg_style(ctx);
                                    Self::draw_fs_image(
                                        ui, image_rect,
                                        state.tex.as_ref(), state.thumb_tex.as_ref(),
                                        state.is_video, state.vst3_waiting_for_video,
                                        state.fs_load_failed, fs_rotation, zp,
                                        free_rot, &bg_style, &state.location_display,
                                    );
                                    // 単一表示時は見開きレイアウトキャッシュを破棄
                                    self.fs_spread_layout = None;
                                }
                                SpreadPair::Double { left, right } => {
                                    self.draw_fs_spread(
                                        ui,
                                        ctx,
                                        image_rect,
                                        left,
                                        right,
                                        state.original_preview_active,
                                    );
                                }
                            }
                        }
                        fs_media_ms = media_t0.elapsed().as_secs_f64() * 1000.0;

                        // ── 消しゴムモード: マスク塗り＋オーバーレイ描画 ──
                        // `is_spread_double` はキー入力ハンドラより前 (フレーム冒頭) で
                        // 計算されるので、見開き中に [E] を押した最初のフレームだけは
                        // `is_spread_double = true` のまま `erase_mode = true` になる
                        // フレーム単位の遷移期間が存在する。この 1 フレームでは見開き
                        // レンダ + 消しゴム overlay を併発させずスキップして、次フレームに
                        // 単一ページ表示で消しゴムを描画する。
                        if self.erase_mode && !is_spread_double && !state.original_preview_active {
                            let zp = self.fs_zoom_pan();
                            self.handle_erase_paint(ctx, image_rect, zp);
                            self.draw_erase_overlay(ui, ctx, image_rect, zp);
                            ctx.request_repaint();
                        } else if self.erase_mode {
                            // 遷移フレームでも次フレームを必ず描画させる (request_repaint)
                            ctx.request_repaint();
                        }

                        let overlay_t0 = std::time::Instant::now();
                        // ── ルーペ (Shift ホールド / M トグル) ──
                        // 見開き・分析・補正モードでは内部で早期 return する。
                        // 消しゴムモードのマスクオーバーレイより上に載せる (最新状態を拡大)。
                        self.draw_fs_loupe_if_active(
                            ui, ctx, full_rect, fs_idx,
                            state.tex.as_ref(), state.thumb_tex.as_ref(),
                            is_spread_double,
                        );

                        // ── 透過背景インジケータ (B キー変更直後のみフェード表示) ──
                        self.draw_fs_transparent_bg_indicator(ui, full_rect);
                        self.draw_original_preview_indicator(ui, full_rect, state.original_preview_active);
                        fs_overlay_ms = overlay_t0.elapsed().as_secs_f64() * 1000.0;

                        // ── 動画 HUD (下部の再生バー + 時刻 + 音量 + シークバー) ──
                        // 入力ハンドリングは handle_image_keys の冒頭で先に呼ばれる
                        // (handle_video_input)。ここでは描画のみ。
                        // Phase 5.6 ミニツールバーはパネル描画 **後** (= 上層) に
                        // 移すため、ここでは draw_video_hud / paused_hint のみ。
                        let hud_t0 = std::time::Instant::now();
                        if state.is_video {
                            self.draw_video_hud(ui, ctx, full_rect, fs_idx);
                            self.draw_video_paused_hint(ui, full_rect, fs_idx);
                            self.sample_video_perf(fs_idx);
                            if self.video_perf_overlay_visible {
                                self.draw_video_perf_overlay(ui, full_rect);
                            }
                        }
                        fs_hud_ms = hud_t0.elapsed().as_secs_f64() * 1000.0;

                        // ── チェックマーク ──
                        if self.checked.contains(&fs_idx) {
                            draw_fs_checkmark(ui, full_rect);
                        }

                        // ── 高解像度読込中インジケーター ──
                        let has_any_tex = state.tex.is_some() || state.thumb_tex.is_some();
                        let pdf_rerendering = self.fs_pending.contains_key(&fs_idx);
                        if state.is_loading && has_any_tex && !pdf_rerendering {
                            ui.painter().text(
                                image_rect.min + egui::vec2(16.0, 16.0),
                                egui::Align2::LEFT_TOP,
                                "高解像度 読込中...",
                                egui::FontId::proportional(14.0),
                                egui::Color32::from_rgba_unmultiplied(220, 220, 220, 180),
                            );
                        }

                        // ── PDF 再レンダリング進捗 ──
                        if self.fs_pending.contains_key(&fs_idx) {
                            let label = if matches!(self.items.get(fs_idx), Some(GridItem::PdfPage { .. })) {
                                "PDF 再レンダリング中..."
                            } else {
                                "読込中..."
                            };
                            let font = egui::FontId::proportional(13.0);
                            let pos = egui::pos2(image_rect.min.x + 12.0, image_rect.max.y - 12.0);
                            let galley = ui.painter().layout_no_wrap(
                                label.to_string(), font.clone(), egui::Color32::WHITE,
                            );
                            let text_rect = egui::Align2::LEFT_BOTTOM
                                .anchor_size(pos, galley.size());
                            let bg = text_rect.expand(4.0);
                            ui.painter().rect_filled(
                                bg, 4.0,
                                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 200),
                            );
                            ui.painter().galley(text_rect.min, galley, egui::Color32::WHITE);
                        }

                        // ── 分析パネル（分析モード時、見開き中は無効）──
                        let panels_t0 = std::time::Instant::now();
                        if analysis_active {
                            let pixels = match self.fs_cache.get(&fs_idx) {
                                Some(FsCacheEntry::Static { pixels, .. }) => {
                                    Some(std::sync::Arc::clone(pixels))
                                }
                                _ => None,
                            };
                            let close_analysis = self.draw_analysis_panel(
                                ui, ctx, full_rect, image_rect, pixels.as_deref(),
                            );
                            if close_analysis {
                                self.reset_analysis_mode();
                            }
                        } else if state.is_video {
                            // 動画は native presenter (独立 HWND) が描画とオーバーレイを担うため、
                            // 通常 eframe ビューポート側ではパネルを描画しない。タイル再オープン
                            // 要求 (S キーの遷移) のリトライだけはここで処理する。
                            #[cfg(windows)]
                            {
                                if self.video_tile_reopen_pending
                                    && self.video_tile_state.is_none()
                                {
                                    let now = std::time::Instant::now();
                                    let deadline =
                                        *self.video_tile_reopen_deadline.get_or_insert_with(|| {
                                            now + std::time::Duration::from_secs(3)
                                        });
                                    if now >= deadline {
                                        self.video_tile_reopen_pending = false;
                                        self.video_tile_reopen_deadline = None;
                                    } else {
                                        let screen = ctx.content_rect().size();
                                        self.toggle_video_tile_mode(fs_idx, screen);
                                        if self.video_tile_state.is_some() {
                                            self.video_tile_reopen_pending = false;
                                            self.video_tile_reopen_deadline = None;
                                        } else {
                                            let retry = std::time::Duration::from_millis(80);
                                            let wait =
                                                deadline.saturating_duration_since(now).min(retry);
                                            ctx.request_repaint_after(wait);
                                        }
                                    }
                                }
                            }
                        } else if adjustment_active {
                            // ── オーバーレイモード: 左パネル + 右パネル 同時表示 ──
                            // 上部ホバーバーと重ならないよう、左パネルは上部バーの下から開始する。
                            let panel_w = crate::ui_adjustment_panel::LEFT_PANEL_WIDTH.min(full_rect.width() * 0.3);
                            let panel_rect = egui::Rect::from_min_max(
                                egui::pos2(full_rect.min.x, full_rect.min.y + TOP_BAR_HEIGHT),
                                egui::pos2(full_rect.min.x + panel_w, full_rect.max.y),
                            );
                            self.draw_adjustment_panel(ui, panel_rect, state.image_dims);
                            // 右側にメタデータパネルも同時表示（show_metadata_panel の状態に関係なく）
                            if !is_spread_double {
                                self.draw_metadata_panel_forced(ui, ctx, full_rect);
                            }
                        } else if !is_spread_double {
                            // ── メタデータパネル（通常モード：TABキー固定 or 右端ホバー）──
                            let right_panel_visible =
                                self.draw_metadata_panel(ui, ctx, full_rect);
                            let _ = right_panel_visible;
                        }
                        fs_panels_ms = panels_t0.elapsed().as_secs_f64() * 1000.0;

                        // ── ホバーバー ──
                        let hover_bar_t0 = std::time::Instant::now();
                        let mut bar_rotate_cw = false;
                        let mut bar_rotate_ccw = false;
                        let spread_before = self.spread_mode;
                        // AI 処理情報を計算（ホバーバーのファイル情報に表示）
                        let ai_info_model_name: String;
                        let ai_upscale_info = if self.ai_upscale_enabled || self.ai_denoise_model.is_some() {
                            ai_info_model_name = self.ai_model_label(fs_idx, false);
                            // 処理後のサイズ
                            if let Some(crate::fs_animation::FsCacheEntry::Static { tex, .. }) =
                                self.ai_upscale_cache.get(&(fs_idx, self.effective_upscale_bg_mode()))
                            {
                                let s = tex.size_vec2();
                                Some((ai_info_model_name.as_str(), s.x as u32, s.y as u32))
                            } else if self.ai_upscale_enabled {
                                if let Some((w, h)) = state.image_dims {
                                    if crate::ai::upscale::should_process(w, h, self.settings.ai_upscale_skip_px) {
                                        Some((ai_info_model_name.as_str(), w * 4, h * 4))
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                        // 消しゴムモード中は上部バーを抑制 (自前の消しゴムパネルと競合させない)。
                        if !self.erase_mode {
                            let saved_nav = nav_delta;
                            let has_page_override = self.adjustment_page_params.contains_key(&fs_idx);
                            // Phase 6: 動画モードかどうかを上ホバーバーに通知する。
                            // 動画なら video_meta (duration, bitrate) と画像 dims (= 動画解像度) を
                            // 動画用 info text 構築のために流す。
                            let is_video_mode = state.is_video;
                            let mut video_dims: Option<(u32, u32)> = None;
                            let mut video_meta: Option<(f64, i64)> = None;
                            #[cfg(windows)]
                            if is_video_mode {
                                if let Some(crate::fs_animation::FsCacheEntry::Video {
                                    player, ..
                                }) = self.fs_cache.get(&fs_idx)
                                {
                                    if let Some(info) = player.info() {
                                        video_dims = Some((info.width, info.height));
                                        video_meta =
                                            Some((info.duration_secs, info.bit_rate_bps));
                                    }
                                }
                            }
                            let display_dims = if is_video_mode {
                                video_dims
                            } else {
                                state.image_dims
                            };
                            #[cfg(windows)]
                            let tile_active = self.video_tile_state.is_some();
                            #[cfg(not(windows))]
                            let tile_active = false;
                            let mut tile_pressed = false;
                            // VST ボタン: 動画モード + VST3 機能有効のときだけ表示
                            let show_vst3_button =
                                cfg!(windows) && is_video_mode && self.settings.vst3_enabled;
                            let vst3_panel_open = self.show_vst3_manager;
                            let mut vst3_pressed = false;
                            Self::draw_fs_hover_bar(
                                ui, ctx, full_rect,
                                &state.location_display,
                                display_dims, state.image_file_size,
                                state.image_downscaled,
                                &mut close_fs, &mut nav_delta,
                                &mut self.show_metadata_panel,
                                false,
                                &mut self.slideshow_playing,
                                &mut self.settings.slideshow_interval_secs,
                                &mut bar_rotate_cw, &mut bar_rotate_ccw,
                                &mut self.analysis_mode,
                                &mut self.spread_mode, &mut self.spread_popup_open,
                                is_spread_double,
                                ai_upscale_info,
                                &mut self.adjustment_mode,
                                has_page_override,
                                state.pdf_content_type,
                                is_video_mode,
                                video_meta,
                                tile_active,
                                &mut tile_pressed,
                                show_vst3_button,
                                vst3_panel_open,
                                &mut vst3_pressed,
                            );
                            // ▦ タイルボタンが押されたら toggle_video_tile_mode に dispatch
                            #[cfg(windows)]
                            if tile_pressed {
                                let screen = ctx.content_rect().size();
                                self.toggle_video_tile_mode(fs_idx, screen);
                            }
                            // VST ボタンが押されたら「管理パネル + 全プラグイン GUI」を一斉トグル。
                            // workspace 全体を「VST3 を見る」「VST3 を畳む」の 2 状態に切り替える。
                            // Compact mode is remembered, but only applied while VST GUIs are visible.
                            #[cfg(windows)]
                            if vst3_pressed {
                                let opening = !self.show_vst3_manager;
                                self.show_vst3_manager = opening;
                                std::sync::Arc::clone(&self.dsp_bridge)
                                    .set_all_guis_visible_async(opening);
                                self.settings.vst3_gui_visible = opening;
                                self.settings.save();
                            }
                            #[cfg(not(windows))]
                            if vst3_pressed {
                                self.show_vst3_manager = !self.show_vst3_manager;
                            }
                            // ホイール/キーで確定した nav_delta を保護
                            if nav_locked { nav_delta = saved_nav; }
                        }
                        fs_hover_bar_ms = hover_bar_t0.elapsed().as_secs_f64() * 1000.0;
                        if bar_rotate_cw { self.rotate_image_cw(fs_idx); }
                        if bar_rotate_ccw { self.rotate_image_ccw(fs_idx); }

                        // ── フルスクリーン用コンテキストメニュー ──
                        if self.show_fs_context_menu(ctx) {
                            close_fs = true;
                        }

                        // ── フルスクリーン左下ステータス表示 ──
                        if self.ai_upscale_enabled || self.ai_denoise_model.is_some() {
                            self.draw_fs_ai_status(ui, fs_idx);
                        }

                        // ── 右上フィードバックトースト ──
                        self.draw_feedback_toast(ui, full_rect, ctx);

                        // 動画ブックマーク名編集ダイアログは native presenter overlay の
                        // 中で描画される (= `native_presenter/overlay_draw.rs::draw_native_*`)。
                        // eframe ビューポートからは描画しない。

                        // ── 中央の境界ヒント (最初/最後の画像です…) ──
                        self.draw_boundary_hint(ui, full_rect, ctx);

                        // ── スロット保存ダイアログ ──
                        self.draw_slot_save_dialog(ctx);

                        // ホバーバーのポップアップからモードが変更された場合
                        if self.spread_mode != spread_before {
                            if let (Some(db), Some(folder)) = (&self.spread_db, &self.current_folder) {
                                let _ = db.set(folder, self.spread_mode, self.settings.default_spread_mode);
                            }
                            if self.spread_mode.is_spread() && self.analysis_mode {
                                self.reset_analysis_mode();
                            }
                            self.adjust_spread_target = crate::app::AdjustSpreadTarget::Left;
                            self.normalize_spread_position();
                        }
                    });
                fs_central_ms = central_t0.elapsed().as_secs_f64() * 1000.0;

                // パネル / HUD が全て非表示で 3 秒以上アイドルならカーソルを隠す。
                // 動画 native presenter 経路は `update_cursor_icon` で
                // `CursorIcon::None` → `SetCursor(None)` に解決する (= 完全非表示)。
                // 静止画 (egui) 経路は winit が `CursorIcon::None` を Windows API で
                // 正しく非表示処理する。VST3 manager 等のダイアログ表示中は
                // `any_dialog_open()` で `fs_ui_is_clean` が false を返すため抑制される。
                //
                // 状態機械:
                // - 入力あり / UI 表示中: `cursor_last_activity = Some(now)`,
                //   `cursor_hidden = false` (idle タイマをリセット)。これにより
                //   一時停止 (HUD 表示中) の間にタイマが古くなり、再開直後に即座に
                //   カーソルが消える事故を防ぐ。
                // - clean かつ idle >= 3 秒、または `cursor_hidden` が立っている:
                //   `CursorIcon::None` を毎フレーム適用 (egui は frame 跨ぎで sticky に
                //   ならないため)、`cursor_hidden = true` をセット。
                {
                    let full_rect = ctx.content_rect();
                    let is_video = state.is_video;
                    let clean = self.fs_ui_is_clean(ctx, full_rect, is_video);
                    if !clean {
                        // UI が出ている間はタイマを today に戻して countdown を停止。
                        self.cursor_last_activity = Some(std::time::Instant::now());
                        self.cursor_hidden = false;
                    }
                    let idle = self
                        .cursor_last_activity
                        .map(|t| t.elapsed().as_secs_f32())
                        .unwrap_or(0.0);
                    let threshold = crate::video::native_presenter::CURSOR_HIDE_IDLE_SECS;
                    if clean && (idle >= threshold || self.cursor_hidden) {
                        ctx.set_cursor_icon(egui::CursorIcon::None);
                        self.cursor_hidden = true;
                    } else if clean {
                        // カウントダウン中: 残時間後に再描画予約してきっかり 3 秒で隠す。
                        let remain = (threshold - idle).max(0.05);
                        ctx.request_repaint_after(std::time::Duration::from_secs_f32(remain));
                    }
                }

                // ── VST3 プラグイン管理ウィンドウ + チェーンエディタ (フルスクリーン中も表示) ──
                // egui::Window はビューポート単位で z-order が独立しているので、
                // フルスクリーンビューポート内で `show_vst3_manager` を呼ぶことで
                // 動画の手前に管理パネルを描画できる。動画分析中に動画を見ながら
                // プラグインを追加・順序入れ替え・バイパス切替・GUI 表示できる。
                #[cfg(windows)]
                {
                    let vst_t0 = std::time::Instant::now();
                    self.show_vst3_manager(ctx);
                    self.vst3_pump_gui_signals();
                    fs_vst_manager_ms = vst_t0.elapsed().as_secs_f64() * 1000.0;
                }

                self.fs_prev_foreground_hwnd = current_foreground_hwnd();
                fs_closure_ms = closure_t0.elapsed().as_secs_f64() * 1000.0;
            },
        );
        let fs_viewport_ms = fs_viewport_t0.elapsed().as_secs_f64() * 1000.0;
        if crate::perf::is_enabled() && fs_viewport_ms > 8.0 {
            let fs_outer_ms = (fs_viewport_ms - fs_closure_ms).max(0.0);
            let fs_closure_tracked_ms = fs_setup_ms + fs_central_ms + fs_vst_manager_ms;
            let fs_closure_unaccounted_ms = (fs_closure_ms - fs_closure_tracked_ms).max(0.0);
            let fs_central_tracked_ms = fs_input_ms
                + fs_media_ms
                + fs_overlay_ms
                + fs_hud_ms
                + fs_panels_ms
                + fs_hover_bar_ms;
            let fs_central_unaccounted_ms = (fs_central_ms - fs_central_tracked_ms).max(0.0);
            let (video_playing, video_pending_frames, video_state, video_seq) =
                match self.fs_cache.get(&fs_idx) {
                    Some(FsCacheEntry::Video { player, .. }) => (
                        player.is_playing(),
                        player.pending_frames() as i64,
                        player.engine_state_code() as i64,
                        player.displayed_frame_seq() as i64,
                    ),
                    _ => (false, -1, -1, -1),
                };
            crate::perf::event(
                "ui",
                "fs_viewport_breakdown",
                None,
                self.input_seq,
                &[
                    ("idx", serde_json::Value::from(fs_idx as i64)),
                    ("total_ms", serde_json::Value::from(fs_viewport_ms)),
                    ("outer_ms", serde_json::Value::from(fs_outer_ms)),
                    ("closure_ms", serde_json::Value::from(fs_closure_ms)),
                    (
                        "closure_unaccounted_ms",
                        serde_json::Value::from(fs_closure_unaccounted_ms),
                    ),
                    (
                        "central_unaccounted_ms",
                        serde_json::Value::from(fs_central_unaccounted_ms),
                    ),
                    ("setup_ms", serde_json::Value::from(fs_setup_ms)),
                    ("central_ms", serde_json::Value::from(fs_central_ms)),
                    ("input_ms", serde_json::Value::from(fs_input_ms)),
                    ("media_ms", serde_json::Value::from(fs_media_ms)),
                    ("overlay_ms", serde_json::Value::from(fs_overlay_ms)),
                    ("hud_ms", serde_json::Value::from(fs_hud_ms)),
                    ("panels_ms", serde_json::Value::from(fs_panels_ms)),
                    ("hover_bar_ms", serde_json::Value::from(fs_hover_bar_ms)),
                    ("vst_manager_ms", serde_json::Value::from(fs_vst_manager_ms)),
                    ("is_video", serde_json::Value::from(fs_state_is_video)),
                    ("gpu_video", serde_json::Value::from(fs_state_gpu_video)),
                    ("video_playing", serde_json::Value::from(video_playing)),
                    (
                        "video_pending_frames",
                        serde_json::Value::from(video_pending_frames),
                    ),
                    ("video_state", serde_json::Value::from(video_state)),
                    ("video_seq", serde_json::Value::from(video_seq)),
                    (
                        "vst3_manager",
                        serde_json::Value::from(self.show_vst3_manager),
                    ),
                ],
            );
        }

        self.fs_viewport_shown = true;

        // ── ナビゲーション & スライドショー処理 ──
        self.handle_fs_navigation(ctx, close_fs, ctrl_nav, nav_delta, jump_to, fs_idx);

        // hint_start_before と一致 = このフレームで再設定されていない
        // (= 境界でない方向への移動、別キー入力、等)。操作があれば即消去。
        // 再設定されていた場合 (= 引き続き境界に突き当たった) はそのまま残す。
        if had_user_input_in_frame {
            let hint_now = self.fs_boundary_hint.map(|h| h.started_at());
            if hint_now.is_some() && hint_now == hint_start_before {
                self.fs_boundary_hint = None;
            }
        }
        self.handle_fs_repaint(ctx, fs_idx, state.is_video);
    }

    // ── 状態準備ヘルパー ────────────────────────────────────────────────

    /// アニメーションフレームを進める（メインコンテキストの時刻を使う）。
    fn advance_animation(&mut self, ctx: &egui::Context, fs_idx: usize) {
        let is_video = matches!(self.items.get(fs_idx), Some(GridItem::Video(_)));
        if is_video {
            return;
        }
        let now = ctx.input(|i| i.time);
        if let Some(FsCacheEntry::Animated {
            frames,
            current_frame,
            next_frame_at,
            ..
        }) = self.fs_cache.get_mut(&fs_idx)
        {
            if now >= *next_frame_at && !frames.is_empty() {
                *current_frame = (*current_frame + 1) % frames.len();
                let delay = frames[*current_frame].1.max(0.02);
                *next_frame_at = now + delay;
            }
        }
    }

    /// 上部ホバーバー・読込中プレースホルダ共通で使う、`idx` 位置の表示用パス文字列。
    /// 通常は `<folder>\<filename>`、ZIP/PDF 内は `<archive> > <entry>` / `<pdf> > Page N`。
    fn location_display_for(&self, idx: usize) -> String {
        let item = self.items.get(idx);
        let filename = item.map(|i| i.name().to_string()).unwrap_or_default();
        let base_folder = self
            .effective_folder()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        compute_location_display(item, &base_folder, &filename)
    }

    /// 見開きの各ページ用: 読込中ブランチが走りそうなときだけパスを計算する。
    /// steady state (fs_cache or thumbnail hit) では空文字列を返し、呼び出し先の
    /// `draw_centered_elided_label` は空なら描画をスキップする。
    fn location_display_for_loading(&self, idx: usize) -> String {
        let has_display_tex = matches!(
            self.fs_cache.get(&idx),
            Some(FsCacheEntry::Static { .. })
                | Some(FsCacheEntry::Animated { .. })
                | Some(FsCacheEntry::Video { .. })
        ) || matches!(
            self.thumbnails.get(idx),
            Some(ThumbnailState::Loaded { .. })
        );
        if has_display_tex {
            String::new()
        } else {
            self.location_display_for(idx)
        }
    }

    /// フルスクリーン描画に必要な状態を事前計算する。
    fn prepare_fullscreen_state(&mut self, ctx: &egui::Context, fs_idx: usize) -> FsFrameState {
        let is_video = matches!(self.items.get(fs_idx), Some(GridItem::Video(_)));
        let separator_text = match self.items.get(fs_idx) {
            Some(GridItem::ZipSeparator { dir_display }) => Some(dir_display.clone()),
            _ => None,
        };
        let is_separator = separator_text.is_some();
        let original_preview_active = self.original_preview_active(ctx, fs_idx);

        let tex: Option<egui::TextureHandle> = if is_video {
            // 動画は native presenter が独立 HWND に描画するため、egui 側で
            // 表示するテクスチャは無い。サムネイル fallback に任せる。
            None
        } else if original_preview_active {
            self.resolve_original_preview_tex(ctx, fs_idx)
        } else {
            // 補正済みキャッシュ（フル解像度）
            let adj_tex = match self.adjustment_cache.get(&fs_idx) {
                Some(FsCacheEntry::Static { tex, .. }) => Some(tex.clone()),
                _ => None,
            };

            // AI 処理有効時（アップスケール or デノイズ）: 処理済みテクスチャ
            let ai_tex = if adj_tex.is_none()
                && (self.ai_upscale_enabled || self.ai_denoise_model.is_some())
            {
                let bg = self.effective_upscale_bg_mode();
                match self.ai_upscale_cache.get(&(fs_idx, bg)) {
                    Some(FsCacheEntry::Static { tex, .. }) => Some(tex.clone()),
                    _ => None,
                }
            } else {
                None
            };

            adj_tex
                .or(ai_tex)
                .or_else(|| match self.fs_cache.get(&fs_idx) {
                    Some(FsCacheEntry::Static { tex, .. }) => Some(tex.clone()),
                    Some(FsCacheEntry::Animated {
                        frames,
                        current_frame,
                        ..
                    }) => frames.get(*current_frame).map(|(h, _)| h.clone()),
                    Some(FsCacheEntry::Video { .. }) => None,
                    Some(FsCacheEntry::Failed) | None => None,
                })
        };

        let fs_load_failed = matches!(self.fs_cache.get(&fs_idx), Some(FsCacheEntry::Failed));

        let thumb_tex = match self.thumbnails.get(fs_idx) {
            Some(ThumbnailState::Loaded { tex, .. }) => Some(tex.clone()),
            _ => None,
        }
        .or_else(|| {
            // ナビ ロック中で新ページのサムネ未準備のときは旧ページのテクスチャを
            // 流用して「ファイル名だけが切り替わる」状態を回避する。サムネが
            // Loaded になった瞬間 `poll_fs_nav_lock` がロックを解除し holdover が
            // 解放される。
            if self.fs_nav_is_locked() {
                self.fs_holdover_tex.clone()
            } else {
                None
            }
        });

        let mut location_display = self.location_display_for(fs_idx);
        // 動画の場合は decode 経路 (HW/SW) と GPU パスを末尾に追記。
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            if let Some(info) = player.info() {
                let hw = if info.hw_decode_active { "HW" } else { "SW" };
                let mut tags = vec![hw.to_string()];
                #[cfg(windows)]
                {
                    if info.gpu_path_active {
                        tags.push("GPU".into());
                    }
                }
                location_display = format!("{}  [{}]", location_display, tags.join("/"));
            }
        }
        // image_dims は常に元画像のサイズを表示する（AI アップスケール後のサイズではない）。
        // AI テクスチャが選ばれている場合でも、元画像のサイズを使う。
        // GPU 上限超過で worker が clamp した画像は `source_dims` に原寸が入っており、
        // ホバー表示はそれを使う。clamp が発動していたら後段で警告マーカーを出す。
        // fs_cache が未到達でも `fs_early_dims` (ヘッダ解析結果) にヒントがあれば使う。
        let (image_dims, image_downscaled): (Option<(u32, u32)>, bool) = {
            match self.fs_cache.get(&fs_idx) {
                Some(FsCacheEntry::Static {
                    tex, source_dims, ..
                }) => {
                    let tex_size = tex.size_vec2();
                    match source_dims {
                        Some([sw, sh]) => {
                            let clamped = (*sw, *sh) != (tex_size.x as usize, tex_size.y as usize);
                            (Some((*sw as u32, *sh as u32)), clamped)
                        }
                        None => (Some((tex_size.x as u32, tex_size.y as u32)), false),
                    }
                }
                Some(FsCacheEntry::Animated {
                    frames,
                    current_frame,
                    ..
                }) => {
                    let dims = frames.get(*current_frame).map(|(h, _)| {
                        let s = h.size_vec2();
                        (s.x as u32, s.y as u32)
                    });
                    (dims, false)
                }
                Some(FsCacheEntry::Video { player, .. }) => {
                    if let Some(info) = player.info() {
                        (Some((info.width, info.height)), false)
                    } else {
                        (None, false)
                    }
                }
                _ => {
                    // まだ fs_cache が埋まっていない段階: ヘッダ解析済みなら原寸ヒントを使う。
                    // 原寸が GPU 上限超なら、本デコード完了後に clamp が発動することが
                    // 事前に判るのでこの時点で⚠ダウンスケール警告を出してよい。
                    if let Some([sw, sh]) = self.fs_early_dims.get(&fs_idx).copied() {
                        let will_clamp =
                            sw > crate::app::MAX_TEXTURE_DIM || sh > crate::app::MAX_TEXTURE_DIM;
                        (Some((sw as u32, sh as u32)), will_clamp)
                    } else {
                        // 最後の頼り: フォールバックサムネイル/テクスチャから寸法を取得。
                        let dims = tex.as_ref().map(|t| {
                            let s = t.size_vec2();
                            (s.x as u32, s.y as u32)
                        });
                        (dims, false)
                    }
                }
            }
        };
        let image_file_size: Option<u64> = self
            .image_metas
            .get(fs_idx)
            .and_then(|m| m.map(|(_, sz)| sz.max(0) as u64));
        let is_loading =
            !is_video && !is_separator && !fs_load_failed && !self.fs_cache.contains_key(&fs_idx);
        #[cfg(windows)]
        let vst3_waiting_for_video = is_video
            && self.vst3_deferred_video_open == Some(fs_idx)
            && self.vst3_startup_load.is_some();
        #[cfg(not(windows))]
        let vst3_waiting_for_video = false;
        #[cfg(windows)]
        if vst3_waiting_for_video {
            location_display = self.vst3_startup_progress_text();
        }

        let pdf_content_type = match self.items.get(fs_idx) {
            Some(GridItem::PdfPage { content_type, .. }) => *content_type,
            _ => None,
        };

        FsFrameState {
            is_video,
            separator_text,
            original_preview_active,
            tex,
            thumb_tex,
            location_display,
            image_dims,
            image_file_size,
            image_downscaled,
            is_loading,
            vst3_waiting_for_video,
            fs_load_failed,
            pdf_content_type,
        }
    }

    /// フルスクリーンビューポートの ViewportBuilder を構築する。
    #[cfg(windows)]
    fn native_video_presenter_hwnd_for_fs(&self, fs_idx: usize) -> Option<u64> {
        match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => {
                let hwnd = player.native_presenter_hwnd();
                (hwnd != 0).then_some(hwnd)
            }
            _ => None,
        }
    }

    #[cfg(windows)]
    fn native_video_backdrop_target_for_fs(&self, fs_idx: usize) -> bool {
        matches!(self.items.get(fs_idx), Some(GridItem::Video(_)))
    }

    #[cfg(windows)]
    fn native_video_presenter_pending_for_fs(&self, fs_idx: usize) -> bool {
        match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => {
                if player.native_presenter_pending() {
                    return true;
                }
                let hwnd = player.native_presenter_hwnd();
                hwnd != 0 && self.native_video_front_synced_hwnd != hwnd
            }
            _ => false,
        }
    }

    #[cfg(windows)]
    fn show_native_video_black_backdrop(&mut self, ctx: &egui::Context, fs_idx: usize) {
        let fs_id = self.fullscreen_viewport_id();
        let fs_builder = self.build_fullscreen_viewport_builder_with_transparency(false);
        let need_show = !self.fs_viewport_shown;
        let mut close_fs = false;
        ctx.show_viewport_immediate(fs_id, fs_builder, |ctx, _class| {
            // Visible な fullscreen viewport なので、native 動画の黒 backdrop 中も
            // IME 状態だけは通常 viewport と同じ入口で更新する。
            self.update_ime_state(ctx);
            if need_show {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            if let Some(keys) = fullscreen_shortcut_event_summary(ctx) {
                let focused = ctx.input(|i| i.viewport().focused).unwrap_or(true);
                crate::logger::log(format!(
                    "[fs-key] source=native-backdrop focused={} foreground=0x{:x} keys={}",
                    focused,
                    current_foreground_hwnd(),
                    keys
                ));
            }
            if !self.ime_input_active() {
                for key in native_video_key_events_from_ctx(ctx) {
                    self.handle_native_video_key_event(ctx, fs_idx, key);
                }
            }
            let close_requested = ctx.input(|i| i.viewport().close_requested());
            let escape_pressed = !self.ime_input_active()
                && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
            if close_requested || escape_pressed {
                close_fs = true;
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(egui::Color32::BLACK))
                .show(ctx, |_ui| {});
        });
        self.fs_viewport_shown = true;
        if close_fs {
            self.close_fullscreen();
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    fn build_fullscreen_viewport_builder(&self) -> egui::ViewportBuilder {
        self.build_fullscreen_viewport_builder_with_transparency(true)
    }

    fn build_fullscreen_viewport_builder_with_transparency(
        &self,
        transparent: bool,
    ) -> egui::ViewportBuilder {
        let center = self.last_outer_rect.map(|r| r.center());
        let ppp = self.last_pixels_per_point;

        let monitor_rect =
            center.and_then(|c| crate::monitor::get_monitor_logical_rect_at(c.x * ppp, c.y * ppp));

        let b = egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_transparent(transparent)
            .with_taskbar(false);
        match monitor_rect {
            Some(rect) => b
                .with_position(rect.min)
                .with_inner_size([rect.width(), rect.height()]),
            None => b.with_fullscreen(true),
        }
    }

    // ── 見開きペアリング ────────────────────────────────────────────────

    /// build_nav_indices の結果をキャッシュして返す。
    fn get_nav_indices(&mut self) -> Vec<usize> {
        if let Some(ref cached) = self.cached_nav_indices {
            return cached.clone();
        }
        let nav = build_nav_indices(&self.items, &self.visible_indices);
        self.cached_nav_indices = Some(nav.clone());
        nav
    }

    /// 現在の見開きモードとインデックスからペア表示を解決する。
    pub(crate) fn resolve_spread_pair(&mut self, idx: usize) -> SpreadPair {
        if !self.spread_mode.is_spread() {
            return SpreadPair::Single;
        }

        let nav = self.get_nav_indices();
        let Some(pos) = nav.iter().position(|&i| i == idx) else {
            return SpreadPair::Single;
        };

        // 表紙モード: pos=0 は常に単独
        if self.spread_mode.has_cover() && pos == 0 {
            return SpreadPair::Single;
        }

        // 横長画像は単独
        if is_landscape(idx, &self.fs_cache, &self.thumbnails) {
            return SpreadPair::Single;
        }

        // ペアリング開始位置: 表紙ありなら pos=1 から、なしなら pos=0 から
        let pair_start = if self.spread_mode.has_cover() { 1 } else { 0 };

        // ペア内の位置を計算 (0-indexed from pair_start)
        let relative = pos - pair_start;
        let is_first_of_pair = relative % 2 == 0;

        // ペア相手の pos を決定
        let partner_pos = if is_first_of_pair { pos + 1 } else { pos - 1 };

        // パートナーが存在しない or 横長の場合は単独
        let partner_idx = match nav.get(partner_pos) {
            Some(&pidx) => pidx,
            None => return SpreadPair::Single,
        };
        if is_landscape(partner_idx, &self.fs_cache, &self.thumbnails) {
            return SpreadPair::Single;
        }

        // 小さい pos のインデックスと大きい pos のインデックス
        let (small_idx, large_idx) = if is_first_of_pair {
            (idx, partner_idx)
        } else {
            (partner_idx, idx)
        };

        // LTR: 左=小, 右=大  /  RTL: 左=大, 右=小
        if self.spread_mode.is_rtl() {
            SpreadPair::Double {
                left: large_idx,
                right: small_idx,
            }
        } else {
            SpreadPair::Double {
                left: small_idx,
                right: large_idx,
            }
        }
    }

    /// 見開きモードでの nav_delta を計算する。
    /// 見開き表示中は2ページ送り、Single表示中は1ページ送り。
    /// Shift が押されている場合は常に1ページ送り。
    pub(crate) fn spread_nav_delta(&mut self, base_delta: i32, shift_held: bool) -> i32 {
        if !self.spread_mode.is_spread() || shift_held {
            return base_delta;
        }
        let fs_idx = match self.fullscreen_idx {
            Some(i) => i,
            None => return base_delta,
        };
        // 現在の表示が Single（横長等）なら1ページ送り
        match self.resolve_spread_pair(fs_idx) {
            SpreadPair::Single => base_delta,
            SpreadPair::Double { .. } => base_delta * 2,
        }
    }

    /// 見開きモード切替後、fullscreen_idx をペアの先頭に正規化する。
    pub(crate) fn normalize_spread_position(&mut self) {
        if !self.spread_mode.is_spread() {
            return;
        }
        let Some(idx) = self.fullscreen_idx else {
            return;
        };
        let nav = self.get_nav_indices();
        let Some(pos) = nav.iter().position(|&i| i == idx) else {
            return;
        };

        let pair_start = if self.spread_mode.has_cover() { 1 } else { 0 };
        if pos < pair_start {
            return; // 表紙位置
        }
        let relative = pos - pair_start;
        if relative % 2 != 0 {
            // ペアの2番目にいるので1番目に戻す
            let new_idx = nav[pos - 1];
            self.open_fullscreen(new_idx);
            self.selected = Some(new_idx);
        }
    }

    // ── キー入力 ────────────────────────────────────────────────────────

    /// フルスクリーン表示中にメインビューポートへ届いたキーを、フルスクリーン操作として処理する。
    ///
    /// VST editor の owner 切り替えや cross-process focus handoff のタイミングによっては、
    /// マウスイベントは fullscreen viewport に届く一方で、キーだけ main viewport に届くことがある。
    /// main 側の通常ショートカットは fullscreen 中にブロックされるため、ここで同じ key handler に通す。
    pub(crate) fn handle_fullscreen_root_key_input(&mut self, ctx: &egui::Context) -> bool {
        let Some(fs_idx) = self.fullscreen_idx else {
            return false;
        };
        let Some(keys) = fullscreen_shortcut_event_summary(ctx) else {
            return false;
        };

        let root_focused = ctx.input(|i| i.viewport().focused).unwrap_or(true);
        crate::logger::log(format!(
            "[fs-key] source=root focused={} foreground=0x{:x} keys={}",
            root_focused,
            current_foreground_hwnd(),
            keys
        ));

        let spread_pair = self.resolve_spread_pair(fs_idx);
        let is_spread_double = matches!(spread_pair, SpreadPair::Double { .. });
        let key_action = self.handle_fs_key_input(ctx, fs_idx, is_spread_double);

        if key_action.nav_delta != 0 {
            self.bump_input_seq(
                "fs_root_key",
                Some(&format!("delta={}", key_action.nav_delta)),
            );
        } else if key_action.ctrl_nav.is_some() {
            self.bump_input_seq("fs_root_ctrl_nav", None);
        } else if key_action.close {
            self.bump_input_seq("fs_root_close_key", None);
        }

        self.handle_fs_navigation(
            ctx,
            key_action.close,
            key_action.ctrl_nav,
            key_action.nav_delta,
            key_action.jump_to,
            fs_idx,
        );
        true
    }

    /// フルスクリーンのキー入力を処理し、アクションを返す。
    fn handle_fs_key_input(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        is_spread_double: bool,
    ) -> FsKeyAction {
        let has_focus = ctx.input(|i| i.viewport().focused).unwrap_or(true);
        let mut action = FsKeyAction {
            close: false,
            nav_delta: 0,
            ctrl_nav: None,
            jump_to: None,
        };

        if !has_focus {
            return action;
        }
        // モーダルダイアログ表示中はキー入力を奪わない
        // (テキスト入力やダイアログ内の Enter/Esc 処理を優先)
        if self.any_modal_dialog_open_for_fullscreen_keys() {
            return action;
        }

        // 消しゴムモード中は専用ショートカットのみ有効にし、通常のフルスクリーンショートカット
        // (矢印ナビ、R/L 回転、I メタデータ等) を無効化する。
        if self.erase_mode {
            return self.handle_erase_keys(ctx, fs_idx);
        }

        // 動画フルスクリーン中は専用キーマップ (Enter=play/pause、Shift+Enter=外部プレイヤー、
        // ←→=シーク、↑↓=音量、M=mute、L=loop) を画像系のキー処理より先に走らせる。
        // Space は **動画モードでも画像と同じ選択トグル** として扱うため、ここでは
        // consume せず、後段 (line ~1941) の image key_space ハンドラに流す
        // (Phase 5.1: 画像/動画混在時のキーアサイン重複を解消)。
        // フルスクリーン用コンテキストメニュー表示中は奪わない (= メニュー側の Enter
        // 選択操作を優先、Codex Phase 5.1 P2 反映)。
        let is_video_fs = matches!(self.items.get(fs_idx), Some(GridItem::Video(_)))
            && self.fs_context_menu_idx.is_none();
        let video_horizontal_arrow_key = is_video_fs
            && ctx.input(|i| {
                i.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Key {
                            key: egui::Key::ArrowLeft | egui::Key::ArrowRight,
                            pressed: true,
                            ..
                        }
                    )
                })
            });
        let video_shift_vertical_key = is_video_fs
            && ctx.input(|i| {
                i.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Key {
                            key: egui::Key::ArrowUp | egui::Key::ArrowDown,
                            pressed: true,
                            modifiers,
                            ..
                        } if modifiers.shift
                    )
                })
            });
        if is_video_fs {
            let video_path = if let Some(GridItem::Video(p)) = self.items.get(fs_idx) {
                Some(p.clone())
            } else {
                None
            };
            self.handle_video_input(ctx, fs_idx, video_path.as_deref());
        }

        // ナビゲーションキーは input_mut で消費して、パネル内ウィジェット（スライダー等）に
        // 奪われないようにする
        let esc = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let shift_held = ctx.input(|i| i.modifiers.shift);
        // 左右キーは上下と分離して処理（RTL 反転のため）
        // Shift+矢印（スプレッドナビ）にも対応するため、修飾キーを問わず消費
        let ctrl_d = ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowDown));
        let ctrl_u = ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowUp));
        // マウス戻る/進む (Extra1/Extra2) を Ctrl+↑/↓ と等価に扱う
        let mouse_back = ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Extra1));
        let mouse_forward = ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Extra2));
        let arrow_right = ctx.input_mut(|i| {
            !video_horizontal_arrow_key
                && (i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight)
                    || i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowRight))
        });
        let arrow_left = ctx.input_mut(|i| {
            !video_horizontal_arrow_key
                && (i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft)
                    || i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowLeft))
        });
        let arrow_down = ctx.input_mut(|i| {
            !video_shift_vertical_key
                && (i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)
                    || i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowDown))
        });
        let arrow_up = ctx.input_mut(|i| {
            !video_shift_vertical_key
                && (i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)
                    || i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowUp))
        });
        let key_home = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Home));
        let key_end = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::End));
        let key_i = ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::NONE, egui::Key::I)
                || i.consume_key(egui::Modifiers::NONE, egui::Key::Tab)
        });
        // Space: スライドショー関連 (変数名の紛らわしさ回避のため key_space)
        let key_space = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Space));
        // S: スライドショー 再生/停止 (旧 P キー、左手で押しやすいよう S に移行)
        let key_s = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::S));
        let key_r = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::R));
        let key_l = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::L));
        let key_z = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Z));
        let key_g = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::G));
        let key_m = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::M));
        let key_e = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::E));
        // B: 透過画像の背景サイクル。消しゴムモードでは ui_erase が B (筆ツール) を既に消費している。
        let key_b_bg = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::B));

        // Shift+F1-F5: 開いている画像が属するコンテナ (フォルダ / ZIP / PDF) に
        // レーティング / Shift+F6: コンテナレーティング解除。
        // current_folder がそのまま親コンテナなので、そちらに書き込めば一覧画面で★絞り込みできる。
        // matches_logically 対策で Shift 版を先に consume する (NONE だと Shift 入りも吸収される)。
        let container_rating_key: Option<u8> =
            ctx.input_mut(|i| crate::ui_helpers::consume_rating_fkey(i, egui::Modifiers::SHIFT));
        if let Some(stars) = container_rating_key
            && self.set_current_folder_rating(stars)
        {
            self.show_container_rating_toast(stars);
        }

        // F1-F5: レーティング 1〜5 / F6: レーティング解除
        let rating_key: Option<u8> =
            ctx.input_mut(|i| crate::ui_helpers::consume_rating_fkey(i, egui::Modifiers::NONE));
        if let Some(stars) = rating_key {
            // Undo 用にフルスクリーン現在ページの before/after を 1 件分積む。
            let before = self.rating_cache.get(&fs_idx).copied().unwrap_or(0);
            if before != stars {
                let summary = if stars == 0 {
                    "★解除".to_string()
                } else {
                    format!("★{stars}")
                };
                self.capture_rating_undo(vec![(fs_idx, before, stars)], summary);
            }
            self.set_rating(fs_idx, stars);
            // レーティング変更でフィルタ境界を跨ぐ可能性があるので visible_indices 再計算。
            self.rebuild_visible_indices();
            if stars == 0 {
                self.show_feedback_toast("[★解除]".to_string());
            } else {
                self.show_feedback_toast(format!("[{}]", "★".repeat(stars as usize)));
            }
        }

        // F7/F8: マスクスロット 1/2 をフルスクリーン表示のまま現ページに適用
        // (消しゴムモードに入らず、1 キーで inpaint までを一気に実行)
        let key_f7 = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F7));
        let key_f8 = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F8));
        if key_f7 {
            self.apply_slot_in_viewing_mode(ctx, 1);
        }
        if key_f8 {
            self.apply_slot_in_viewing_mode(ctx, 2);
        }

        // V キー (VST3 プラグイン GUI トグル) は撤去した。理由は app.rs 同箇所参照。
        // フルスクリーン中はホバーバーの "VST" ボタンから管理パネルを開く運用。

        // 消しゴムモード中は ui_erase が先に Ctrl+Z を吸収する。
        self.handle_meta_undo_keys(ctx);

        // 見開きモード切替 (1-5 キー)
        let key_1 = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Num1));
        let key_2 = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Num2));
        let key_3 = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Num3));
        let key_4 = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Num4));
        let key_5 = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Num5));

        // U / Shift+U / Alt+U: AI アップスケールモデル サイクル (次 / 前 / なしリセット)
        // 注意: egui の consume_key は matches_logically で判定されるため、Modifiers::NONE が
        // Shift/Alt を伴う入力まで吸収する。具体的な修飾子から先に consume する必要がある。
        let key_u_alt = ctx.input_mut(|i| i.consume_key(egui::Modifiers::ALT, egui::Key::U));
        let key_u_shift = ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::U));
        let key_u = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::U));
        // N キー: AI デノイズサイクル
        let key_n = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::N));
        // P / Shift+P / Alt+P: ポストフィルタ (レトロ系) サイクル (次 / 前 / なしリセット)
        // Ctrl+F はグリッドで検索に使っているため避けて Alt 修飾を採用。
        // 同様に Alt+P → Shift+P → P の順で consume (matches_logically 対策)。
        let key_p_alt = ctx.input_mut(|i| i.consume_key(egui::Modifiers::ALT, egui::Key::P));
        let key_p_shift = ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::P));
        let key_p = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::P));

        // Ctrl+数字キー: 保存スロットからロード
        // (Shift+数字はキー配列によって記号化され egui::Key::Num1 等にマッチしないため CTRL を採用)
        let slot_keys: [bool; 10] = [
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Num1)),
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Num2)),
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Num3)),
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Num4)),
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Num5)),
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Num6)),
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Num7)),
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Num8)),
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Num9)),
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Num0)),
        ];

        // Ctrl+Backspace / Q: 現在ページの個別補正設定を解除 (標準値に戻す)
        // Q は片手で押しやすいショートカット (補正パネルでの操作中に素早く元に戻したい用途)
        let clear_page_key = ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::CTRL, egui::Key::Backspace)
                || i.consume_key(egui::Modifiers::NONE, egui::Key::Q)
        });

        // 見開きモード切替 + フィードバック表示
        let new_spread = if key_1 {
            Some(SpreadMode::Single)
        } else if key_2 {
            Some(SpreadMode::Ltr)
        } else if key_3 {
            Some(SpreadMode::LtrCover)
        } else if key_4 {
            Some(SpreadMode::Rtl)
        } else if key_5 {
            Some(SpreadMode::RtlCover)
        } else {
            None
        };

        if let Some(mode) = new_spread {
            if mode != self.spread_mode {
                self.spread_mode = mode;
                self.spread_popup_open = false;
                self.adjust_spread_target = crate::app::AdjustSpreadTarget::Left;
                // DB に保存
                if let (Some(db), Some(folder)) = (&self.spread_db, &self.current_folder) {
                    let _ = db.set(folder, mode, self.settings.default_spread_mode);
                }
                // 分析モードを解除 (post-filter バイパスも戻す)
                if mode.is_spread() && self.analysis_mode {
                    self.reset_analysis_mode();
                }
                // ページ位置を正規化
                self.normalize_spread_position();
            }
            // フィードバック表示
            let key_num = if key_1 {
                1
            } else if key_2 {
                2
            } else if key_3 {
                3
            } else if key_4 {
                4
            } else {
                5
            };
            self.show_feedback_toast(format!("[{}:{}]", key_num, mode.label()));
        }

        // U キー: AI アップスケールモデルをサイクル
        // 現在効いているスコープ (個別 > お気に入り標準 > 標準) を書き換える。
        if key_u || key_u_shift || key_u_alt {
            let scope = self.resolve_adjust_scope(fs_idx);
            let mut params = self.effective_params(fs_idx).clone();
            let items = crate::adjustment::upscale_menu_items();
            let cur = items
                .iter()
                .position(|(_, k)| match (k, params.upscale_model.as_deref()) {
                    (None, None) => true,
                    (Some(a), Some(b)) => *a == b,
                    _ => false,
                })
                .unwrap_or(0);
            let next = if key_u_alt {
                0
            } else if key_u_shift {
                (cur + items.len() - 1) % items.len()
            } else {
                (cur + 1) % items.len()
            };
            let (label, key) = items[next];
            params.upscale_model = key.map(|s| s.to_string());
            // 切替先がアップスケール **有効** で、かつ画像サイズが
            // `ai_upscale_skip_px` 閾値以上なら処理がスキップされる。
            // 「切り替えたのに見た目が変わらない」違和感を避けるため
            // トーストに 2 行目で明示する。
            let mut toast = format!("[U:{}アップスケール {}]", scope.label(), label);
            if key.is_some()
                && let Some(crate::fs_animation::FsCacheEntry::Static { pixels, .. }) =
                    self.fs_cache.get(&fs_idx)
            {
                let w = pixels.size[0] as u32;
                let h = pixels.size[1] as u32;
                let threshold = self.settings.ai_upscale_skip_px;
                if !crate::ai::upscale::should_process(w, h, threshold) {
                    toast.push_str(&format!(
                        "\n(解像度が高いためアップスケール処理無効: {w}×{h} ≥ {threshold}px)"
                    ));
                }
            }
            self.show_feedback_toast(toast);
            self.capture_adjust_full(format!("AI アップスケール: {label}"), |app| {
                app.write_params_for_scope(fs_idx, scope, params);
                app.clear_all_adjustment_and_ai_caches(fs_idx);
            });
        }

        // N キー: AI デノイズをトグル
        if key_n {
            let scope = self.resolve_adjust_scope(fs_idx);
            let mut params = self.effective_params(fs_idx).clone();
            if params.denoise_model.is_some() {
                params.denoise_model = None;
                self.show_feedback_toast(format!("[N:{}デノイズ OFF]", scope.label()));
            } else {
                params.denoise_model =
                    Some(crate::ai::ModelKind::DenoiseRealplksr.as_str().to_string());
                self.show_feedback_toast(format!("[N:{}デノイズ ON]", scope.label()));
            }
            self.capture_adjust_full("AI デノイズの切替".to_string(), |app| {
                app.write_params_for_scope(fs_idx, scope, params);
                app.clear_all_adjustment_and_ai_caches(fs_idx);
            });
        }

        // P / Shift+P / Alt+P: ポストフィルタの次/前/なしへ切替。
        // AI 再実行は発生させないため色調キャッシュのみクリア。
        if key_p || key_p_shift || key_p_alt {
            let scope = self.resolve_adjust_scope(fs_idx);
            let mut params = self.effective_params(fs_idx).clone();
            let all = crate::adjustment::PostFilter::ALL;
            let cur = all
                .iter()
                .position(|f| *f == params.post_filter)
                .unwrap_or(0);
            let next_idx = if key_p_alt {
                0
            } else if key_p_shift {
                (cur + all.len() - 1) % all.len()
            } else {
                (cur + 1) % all.len()
            };
            let next = all[next_idx];
            params.post_filter = next;
            self.show_feedback_toast(format!("[P: {} / {}]", scope.label(), next.display_label()));
            self.capture_adjust_full(
                format!("ポストフィルタ: {}", next.display_label()),
                |app| {
                    app.write_params_for_scope(fs_idx, scope, params);
                    match scope {
                        AdjustScope::PageOverride => app.clear_adjustment_caches(fs_idx),
                        AdjustScope::FavoriteDefault(_) | AdjustScope::Global => {}
                    }
                },
            );
        }

        // Ctrl+数字キー: 保存スロットを現在ページに適用 (= ページ個別化)
        for (slot_idx, &pressed) in slot_keys.iter().enumerate() {
            if pressed {
                self.capture_adjust_full(
                    format!(
                        "スロット{}を適用",
                        crate::adjustment::slot_key_label(slot_idx)
                    ),
                    |app| app.apply_slot_to_current_page(slot_idx),
                );
            }
        }

        // Ctrl+Backspace: 個別設定があれば解除、なければフィードバックのみ
        if clear_page_key {
            if self.adjustment_page_params.contains_key(&fs_idx) {
                self.capture_adjust_full("個別設定の解除".to_string(), |app| {
                    app.clear_page_params(fs_idx)
                });
                self.show_feedback_toast("[個別設定を解除]".to_string());
            } else {
                self.show_feedback_toast("[個別設定なし]".to_string());
            }
        }

        if esc {
            action.close = true;
        }
        // 見開きダブル表示中は I/Z/R/L を無効化
        if key_i && !is_spread_double {
            self.show_metadata_panel = !self.show_metadata_panel;
        }
        if key_z && !is_spread_double {
            if self.analysis_mode {
                // 分析→通常: ズーム/パンを引き継ぐ
                self.fs_zoom = self.analysis_zoom;
                self.fs_pan = self.analysis_pan;
                self.reset_analysis_mode();
            } else {
                // 通常→分析: ズーム/パンを引き継ぐ
                self.analysis_zoom = self.fs_zoom;
                self.analysis_pan = self.fs_pan;
                self.analysis_mode = true;
                self.enter_analysis_mode_bypass();
                // 補正パネルと排他
                self.adjustment_mode = false;
            }
        }
        if self.analysis_mode && !is_spread_double {
            if key_g {
                self.analysis_grayscale = !self.analysis_grayscale;
            }
            if key_m {
                self.analysis_mosaic_grid = !self.analysis_mosaic_grid;
                if self.analysis_mosaic_grid {
                    self.analysis_guide_drag = None;
                }
            }
        } else if key_m && !self.adjustment_mode {
            // M: ルーペ表示のトグル (分析モード外でのみ。分析モードでは既存のモザイクグリッド操作)
            self.fs_loupe_locked = !self.fs_loupe_locked;
            self.show_feedback_toast(if self.fs_loupe_locked {
                "[ルーペ ON]".to_string()
            } else {
                "[ルーペ OFF]".to_string()
            });
        }

        // B: 透過画像の背景サイクル (分析・補正・動画モード外)。
        // 消しゴムモードは別ブランチ (handle_erase_keys) で処理済みのためここには来ない。
        // 通常: 黒 → 白 → 市松 の 3 モード循環。
        // AI アップスケール有効時 (composite-first): 市松は出力にパターンが焼き込まれて崩れるので
        // 黒/白の 2 モード循環に制限する。デノイズのみの場合はアルファ保持パスを通るので市松 OK。
        if key_b_bg && !self.analysis_mode && !self.adjustment_mode {
            let modulo: u8 = if self.ai_upscale_enabled { 2 } else { 3 };
            self.fs_transparent_bg_mode = (self.fs_transparent_bg_mode + 1) % modulo;
            self.fs_transparent_bg_indicator_until =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(1200));
            let label = transparent_bg_toast(self.fs_transparent_bg_mode);
            self.show_feedback_toast(label.to_string());
        }

        // E: 消しゴムモード切り替え (分析・補正中は無効)。
        // 見開き中の起動は `enter_erase_mode` が一時的に Single に切り替えて
        // 左ページを編集対象にする (Apply / Cancel で見開き状態に戻る)。
        if key_e && !self.analysis_mode && !self.adjustment_mode {
            if self.erase_mode {
                // 2回目のE: inpaint実行
                self.execute_erase_inpaint(ctx, fs_idx);
            } else {
                // 1回目のE: マスクモード開始
                self.enter_erase_mode(fs_idx);
            }
        }

        // S: スライドショー開始/停止トグル (旧 P、左手で押しやすいよう S へ移行)
        // 開始時のみ、現在ページが画像系アイテム (Image/ZipImage/PdfPage) かを確認する。
        // ZipSeparator など非画像アイテム上では開始させない (停止操作は常に許可)。
        if key_s {
            if self.slideshow_playing {
                self.slideshow_playing = false;
            } else if matches!(
                self.items.get(fs_idx),
                Some(GridItem::Image(_))
                    | Some(GridItem::ZipImage { .. })
                    | Some(GridItem::PdfPage { .. })
            ) {
                self.slideshow_playing = true;
                self.slideshow_next_at = std::time::Instant::now()
                    + std::time::Duration::from_secs_f32(self.settings.slideshow_interval_secs);
            }
        }

        // Space: スライドショー中→停止、停止中→画像をチェック
        if key_space {
            if self.slideshow_playing {
                self.slideshow_playing = false;
            } else {
                match self.items.get(fs_idx) {
                    Some(GridItem::Image(_))
                    | Some(GridItem::Video(_))
                    | Some(GridItem::ZipImage { .. })
                    | Some(GridItem::PdfPage { .. }) => {
                        if self.checked.contains(&fs_idx) {
                            self.checked.remove(&fs_idx);
                        } else {
                            self.checked.insert(fs_idx);
                        }
                    }
                    _ => {}
                }
            }
        }
        if key_r && !is_spread_double {
            self.rotate_image_cw(fs_idx);
        }
        if key_l && !is_spread_double {
            self.rotate_image_ccw(fs_idx);
        }

        // ── ナビゲーション ──
        // RTL モードでは左右キーの意味を反転
        let rtl = self.spread_mode.is_rtl();
        let nav_next = (arrow_right && !rtl) || (arrow_left && rtl) || arrow_down;
        let nav_prev = (arrow_left && !rtl) || (arrow_right && rtl) || arrow_up;

        if nav_next && !ctrl_d {
            action.nav_delta = self.spread_nav_delta(1, shift_held);
            self.slideshow_playing = false;
        }
        if nav_prev && !ctrl_u {
            action.nav_delta = self.spread_nav_delta(-1, shift_held);
            self.slideshow_playing = false;
        }
        if ctrl_d || mouse_forward {
            action.ctrl_nav = Some(1);
        }
        if ctrl_u || mouse_back {
            action.ctrl_nav = Some(-1);
        }

        if key_home {
            if let Some(first) =
                crate::ui_helpers::boundary_navigable_idx(&self.items, &self.visible_indices, false)
            {
                if first != fs_idx {
                    action.jump_to = Some(first);
                    self.slideshow_playing = false;
                } else {
                    self.fs_boundary_hint = Some(FsBoundaryHint::Edge {
                        at_end: false,
                        at: std::time::Instant::now(),
                    });
                }
            }
        }
        if key_end {
            if let Some(last) =
                crate::ui_helpers::boundary_navigable_idx(&self.items, &self.visible_indices, true)
            {
                if last != fs_idx {
                    action.jump_to = Some(last);
                    self.slideshow_playing = false;
                } else {
                    self.fs_boundary_hint = Some(FsBoundaryHint::Edge {
                        at_end: true,
                        at: std::time::Instant::now(),
                    });
                }
            }
        }

        action
    }

    // ── ホイール & クリック ──────────────────────────────────────────────

    /// ホイールとクリックを処理し、(nav_delta, close) を返す。
    fn handle_fs_wheel_and_click(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        state: &FsFrameState,
        is_spread_double: bool,
        prev_foreground_hwnd: usize,
    ) -> (i32, bool) {
        let mut nav_delta = 0i32;
        let mut close = false;

        // VST editor windows are bridge-process native windows. In the
        // cross-process owner-popup case, egui can still report this viewport as
        // focused while Win32 sends keyboard input to the VST editor. Check the
        // actual foreground HWND when the user clicks back into fullscreen.
        let (fullscreen_primary_event, viewport_focused) = ctx.input(|i| {
            let focused = i.viewport().focused.unwrap_or(true);
            let primary_event = i.pointer.primary_down()
                || i.pointer.primary_pressed()
                || i.pointer.primary_released();
            let in_fullscreen = i
                .pointer
                .interact_pos()
                .map(|p| full_rect.contains(p))
                .unwrap_or(false);
            (primary_event && in_fullscreen, focused)
        });
        if fullscreen_primary_event {
            let target = native_window_under_cursor_focus_target();
            let previous_foreign_foreground = prev_foreground_hwnd != 0
                && target.target_hwnd != 0
                && prev_foreground_hwnd != target.target_hwnd;
            let current_foreign_foreground = target.foreground_hwnd != 0
                && target.target_hwnd != 0
                && target.foreground_hwnd != target.target_hwnd;
            let vst_gui_visible =
                cfg!(windows) && self.settings.vst3_enabled && self.settings.vst3_gui_visible;
            let focus_restore_click = previous_foreign_foreground || vst_gui_visible;
            let should_claim_native_focus =
                focus_restore_click && (previous_foreign_foreground || current_foreign_foreground);
            let claim_debounced = self
                .fs_last_native_focus_claim_at
                .map(|t| t.elapsed() < std::time::Duration::from_millis(100))
                .unwrap_or(false);
            let focus = if should_claim_native_focus && !claim_debounced {
                self.fs_last_native_focus_claim_at = Some(std::time::Instant::now());
                claim_native_window_focus(target.target_hwnd)
            } else {
                NativeFocusClaim {
                    foreground_hwnd: target.foreground_hwnd,
                    post_foreground_hwnd: target.foreground_hwnd,
                    target_hwnd: target.target_hwnd,
                    set_foreground_ok: false,
                    attach_thread_input_ok: false,
                    set_active_ok: false,
                    set_focus_ok: false,
                }
            };
            if previous_foreign_foreground
                || current_foreign_foreground
                || should_claim_native_focus
                || claim_debounced
            {
                crate::logger::log(format!(
                    "[fs-focus] prev_foreground=0x{:x} foreground=0x{:x} post_foreground=0x{:x} fullscreen=0x{:x} prev_foreign={} current_foreign={} vst_gui_visible={} viewport_focused={} suppress={} native_claim={} claim_debounced={} set_foreground={} attach_thread_input={} set_active={} set_focus={}",
                    prev_foreground_hwnd,
                    focus.foreground_hwnd,
                    focus.post_foreground_hwnd,
                    focus.target_hwnd,
                    previous_foreign_foreground,
                    current_foreign_foreground,
                    vst_gui_visible,
                    viewport_focused,
                    focus_restore_click,
                    should_claim_native_focus && !claim_debounced,
                    claim_debounced,
                    focus.set_foreground_ok,
                    focus.attach_thread_input_ok,
                    focus.set_active_ok,
                    focus.set_focus_ok
                ));
            }
            if focus_restore_click {
                if should_claim_native_focus && !claim_debounced {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                self.fs_focus_regained_at = Some(std::time::Instant::now());
                self.fs_suppress_primary_until_release = true;
                return (0, false);
            }
        }

        // ── ホイール ──
        // パネル領域内ではホイールナビゲーションを抑制
        let panel_w = METADATA_PANEL_WIDTH.min(full_rect.width() * 0.5);
        let panel_left = full_rect.max.x - panel_w;
        let hover_threshold = full_rect.max.x - full_rect.width() * 0.25;
        let has_right_panel = self.show_metadata_panel;
        let left_panel_w =
            crate::ui_adjustment_panel::LEFT_PANEL_WIDTH.min(full_rect.width() * 0.3);
        let cursor_in_panel = ctx.input(|i| {
            i.pointer
                .hover_pos()
                .map(|p| {
                    let in_right = p.x > panel_left
                        && p.y >= 60.0
                        && (has_right_panel || p.x > hover_threshold);
                    let in_left =
                        self.adjustment_mode && p.x < full_rect.min.x + left_panel_w && p.y >= 60.0;
                    in_right || in_left
                })
                .unwrap_or(false)
        });

        // 左端・上端・右端のホバーでオーバーレイ（上バー＋左パネル＋右パネル）を同時表示/非表示
        // 消しゴムモード中は自前のパネルを左端に描いているためエッジ発火を抑制する。
        // 加えて、消しゴムモードに入る前から adjustment_mode が立っていると、消しゴムパネルが
        // 左端を占有している間 edge_hover が常に true 扱いになり off へ遷移できないので、
        // 強制的に落とす。
        if self.erase_mode {
            self.adjustment_mode = false;
        } else {
            let edge_hover = ctx.input(|i| {
                i.pointer
                    .hover_pos()
                    .map(|p| {
                        p.y < 60.0  // 上端
                    || p.x < full_rect.min.x + full_rect.width() * 0.05  // 左端5%
                    || p.x > full_rect.max.x - full_rect.width() * 0.05 // 右端5%
                    })
                    .unwrap_or(false)
            });
            if edge_hover && !self.analysis_mode {
                self.adjustment_mode = true;
            } else if !cursor_in_panel
                && !edge_hover
                && self.adjustment_mode
                && !self.adjustment_dragging
            {
                self.adjustment_mode = false;
            }
        }

        // ── 中ボタン (ホイール押し込み) ドラッグでズーム ──
        // キーボードを使わず右手のマウスだけで拡大縮小したい用途向け。
        // 分析モード中でも同じ操作感で動かす (書き戻し先はドラッグ開始時の mode で固定)。
        // パネル上で「開始」された中ボタンは無視して下流に流すが、既にドラッグが
        // 走っているときはカーソルがパネルを通過しても継続させる (UX のブレ防止)。
        if !cursor_in_panel || self.fs_middle_zoom_drag.is_some() {
            self.handle_middle_drag_zoom(ctx, full_rect);
        }

        // 動画タイルモード中でも Ctrl なし Wheel は前後アイテム移動に使う。
        // Ctrl+Wheel はタイル overlay 側の列数切替に渡すため、ここでは消費しない。
        #[cfg(windows)]
        let in_video_tile = self.video_tile_state.is_some();
        #[cfg(not(windows))]
        let in_video_tile = false;
        let wheel_y = ctx.input(|i| i.raw_scroll_delta.y);
        let ctrl_held = ctx.input(|i| i.modifiers.ctrl);
        let handle_wheel_here = !cursor_in_panel && (!in_video_tile || !ctrl_held);
        if wheel_y.abs() > 0.5 && handle_wheel_here {
            ctx.input_mut(|i| {
                i.raw_scroll_delta = egui::Vec2::ZERO;
                i.smooth_scroll_delta = egui::Vec2::ZERO;
                i.events
                    .retain(|e| !matches!(e, egui::Event::MouseWheel { .. }));
            });
            // 消しゴムモード: 筆/直線ツールでは修飾なしホイールで太さ調整
            // (Ctrl+ホイールは通常のズームに残す)
            if !ctrl_held
                && self.erase_mode
                && matches!(
                    self.erase_tool,
                    crate::app::EraseTool::Brush | crate::app::EraseTool::Line
                )
            {
                let max_r = self.erase_mask_size[0].max(self.erase_mask_size[1]) as f32 / 20.0;
                let factor = if wheel_y > 0.0 { 1.1 } else { 1.0 / 1.1 };
                match self.erase_tool {
                    crate::app::EraseTool::Brush => {
                        self.erase_brush_radius =
                            (self.erase_brush_radius * factor).clamp(1.0, max_r);
                    }
                    crate::app::EraseTool::Line => {
                        self.erase_line_width = (self.erase_line_width * factor).clamp(1.0, max_r);
                    }
                    _ => {}
                }
                return (0, false); // ホイールを消費したので終了
            }
            if self.analysis_mode {
                // 分析モード: ホイールでズーム
                let mouse = ctx.input(|i| i.pointer.hover_pos());
                let image_rect = analysis_image_rect(full_rect);
                let changed = Self::apply_wheel_zoom(
                    &mut self.analysis_zoom,
                    &mut self.analysis_pan,
                    wheel_y,
                    mouse,
                    image_rect.center(),
                );
                if changed {
                    self.maybe_rerender_pdf(self.analysis_zoom);
                }
            } else {
                if ctrl_held {
                    // 通常モード: Ctrl+ホイールでズーム
                    let mouse = ctx.input(|i| i.pointer.hover_pos());
                    let changed = Self::apply_wheel_zoom(
                        &mut self.fs_zoom,
                        &mut self.fs_pan,
                        wheel_y,
                        mouse,
                        full_rect.center(),
                    );
                    if changed {
                        self.maybe_rerender_pdf(self.fs_zoom);
                    }
                } else if !self.erase_mode {
                    let base = if wheel_y < 0.0 { 1 } else { -1 };
                    nav_delta = self.spread_nav_delta(base, false);
                }
            }
        }

        // ── クリック & ドラッグ ──
        let fs_response = ui.interact(
            full_rect,
            egui::Id::new("fs_click"),
            egui::Sense::click_and_drag(),
        );
        // ── フォーカス復帰クリックの抑制 ──
        // 他アプリから戻ってきた直後に押された左ボタンはナビ目的ではないとみなし、
        // 押下〜離すまでの全期間にわたってアプリ側の左クリック処理を無効化する。
        // グレースを跨いだ正常クリックには影響しないよう、状態ベースで追跡する。
        const FOCUS_RESTORE_GRACE: std::time::Duration = std::time::Duration::from_millis(300);
        let (primary_down, primary_released) =
            ctx.input(|i| (i.pointer.primary_down(), i.pointer.primary_released()));
        if let Some(t) = self.fs_focus_regained_at {
            if t.elapsed() >= FOCUS_RESTORE_GRACE {
                self.fs_focus_regained_at = None;
            } else if !self.fs_suppress_primary_until_release && primary_down {
                self.fs_suppress_primary_until_release = true;
                self.fs_focus_regained_at = None;
            }
        }
        // フレーム冒頭の状態をスナップ。release フレームでは primary_released と
        // fs_response.clicked() が同一フレームで立つので、フラグのクリアを先に
        // 行うと抑制対象のクリックがそのままナビを走らせてしまう。判定は
        // スナップショットで行い、クリアは左クリック分岐が終わった後に回す。
        let suppress_this_frame = self.fs_suppress_primary_until_release;
        if suppress_this_frame {
            // フォーカス復帰クリック: 左ボタン経由の分岐をすべてスキップ。
            // 右クリック処理 (下の secondary ブロック) は別系統なので走らせる
        } else if self.erase_mode {
            // 消しゴムモード: 左クリック/ドラッグはマスク塗りに使うためナビ無効化
        } else if self.analysis_mode {
            // 分析モード: 左クリックでのナビを無効化（パン用のドラッグは analysis_panel 側）
            // ダブルクリックでズームリセット
            if fs_response.double_clicked() {
                self.analysis_zoom = 1.0;
                self.analysis_pan = egui::Vec2::ZERO;
                self.maybe_rerender_pdf(1.0);
            }
            // 右クリックは analysis_panel 側で処理
        } else {
            // ── 通常モード: ドラッグ操作 ──
            let mods = ctx.input(|i| i.modifiers);
            let primary_pressed = fs_response.drag_started_by(egui::PointerButton::Primary);
            let primary_down = fs_response.dragged_by(egui::PointerButton::Primary);
            let primary_released = fs_response.drag_stopped_by(egui::PointerButton::Primary);
            let pointer_pos = ctx.input(|i| i.pointer.hover_pos());

            // 見開き 2 ページ表示中はフリー回転が描画に反映されないため、Ctrl+ドラッグ回転を無効化する
            if mods.ctrl && !is_spread_double {
                // Ctrl+ドラッグ → 回転
                if primary_pressed {
                    if let Some(pos) = pointer_pos {
                        self.fs_rotation_drag_start = Some((pos, self.fs_free_rotation));
                    }
                } else if primary_down {
                    if let Some((start_pos, start_rot)) = self.fs_rotation_drag_start {
                        if let Some(pos) = pointer_pos {
                            let center = full_rect.center() + self.fs_pan;
                            let start_angle =
                                (start_pos.y - center.y).atan2(start_pos.x - center.x);
                            let cur_angle = (pos.y - center.y).atan2(pos.x - center.x);
                            self.fs_free_rotation = start_rot + (cur_angle - start_angle);
                        }
                    }
                }
            } else if self.fs_zoom > ZOOM_NEAR_ONE
                || self.fs_free_rotation.abs() > TRANSFORM_EPSILON
            {
                // ズームまたは回転中: ドラッグでパン
                if primary_pressed {
                    if let Some(pos) = pointer_pos {
                        self.fs_pan_drag_start = Some((pos, self.fs_pan));
                    }
                } else if primary_down {
                    if let Some((start_pos, start_pan)) = self.fs_pan_drag_start {
                        if let Some(pos) = pointer_pos {
                            self.fs_pan = start_pan + (pos - start_pos);
                        }
                    }
                }
            }
            if primary_released {
                self.fs_pan_drag_start = None;
                self.fs_rotation_drag_start = None;
            }

            // ダブルクリック → ズーム/パン/回転リセット
            let has_transform = self.fs_zoom > ZOOM_NEAR_ONE
                || self.fs_free_rotation.abs() > TRANSFORM_EPSILON
                || self.fs_pan.length_sq() > PAN_EPSILON_SQ;
            if fs_response.double_clicked() && has_transform {
                self.fs_zoom = 1.0;
                self.fs_pan = egui::Vec2::ZERO;
                self.fs_free_rotation = 0.0;
                self.maybe_rerender_pdf(1.0);
            } else if !has_transform && self.fs_context_menu_idx.is_none() {
                // 変形なし: 従来の動画/画像クリック動作（コンテキストメニュー表示中は無効）
                let was_dragging = fs_response.dragged() && fs_response.drag_delta().length() > 3.0;
                if !was_dragging {
                    if state.is_video {
                        // 動画: クリックで再生/一時停止トグル (一般的な動画プレイヤー慣例)。
                        // 外部プレイヤーで開きたい場合は Shift+Enter。
                        // ただし下部 HUD バーの矩形内のクリックは play/pause / シーク /
                        // 音量等のウィジェット側で処理させたいので除外する。
                        // Phase 5.4 追加: 左ジャンプパネル / 右メタ情報パネルの上で
                        // クリックしたら toggle_play しない (Codex P5.4 M1 反映)。
                        // Phase 5.5 追加: タイルモード中はオーバーレイのタイルクリック
                        // で seek + close を行うため、background catch-all は完全抑止
                        // (Codex P5.5 H1 反映)。
                        // Phase 7.I 追加: 一時停止中の中央 2 ボタン (最初から / 続きから)
                        // の領域を除外。
                        let tile_active = self.video_tile_state.is_some();
                        let pos_opt = fs_response.interact_pointer_pos();
                        // HUD がフェード中・完全フェード後はクリック判定を映像本体に通す。
                        // 描画されている (= 視認可能な) HUD 上のクリックだけ吸収する。
                        let hud_visible = self.video_hud_visible_factor > 0.05;
                        let in_hud = hud_visible
                            && pos_opt
                                .map(|p| video_hud_rect(full_rect).contains(p))
                                .unwrap_or(false);
                        let in_video_panel = pos_opt
                            .map(|p| {
                                let left_thresh = full_rect.min.x + full_rect.width() * 0.25;
                                let right_thresh = full_rect.max.x - full_rect.width() * 0.25;
                                (p.x < left_thresh || p.x > right_thresh)
                                    && p.y >= full_rect.min.y + 44.0
                            })
                            .unwrap_or(false);
                        // 中央 2 ボタン (一時停止時のみ描画) の領域だけを除外。
                        // 描画条件は native_presenter::draw_native_center_pause_controls
                        // (overlay_draw.rs) と完全に揃える。frame_step 中はボタン非表示
                        // なので除外しない (= ボタン跡地のクリックで toggle_play() が走り
                        // 再生再開できる)。
                        let center_buttons_visible = self
                            .fullscreen_idx
                            .and_then(|idx| self.fs_video_player(idx))
                            .map(|p| {
                                !p.is_playing()
                                    && !p.is_frame_step_active()
                                    && p.displayed_frame_seq() > 0
                            })
                            .unwrap_or(false);
                        let in_center_buttons = if center_buttons_visible {
                            pos_opt
                                .map(|p| {
                                    let cx = full_rect.center().x;
                                    let cy = full_rect.center().y;
                                    // overlay_draw.rs::draw_native_center_pause_controls
                                    // の rect に揃える (radius=56, gap=34, 各 112x112)。
                                    let radius = 56.0_f32;
                                    let gap = 34.0_f32;
                                    let left_rect = egui::Rect::from_center_size(
                                        egui::pos2(cx - radius - gap * 0.5, cy),
                                        egui::vec2(radius * 2.0, radius * 2.0),
                                    );
                                    let right_rect = egui::Rect::from_center_size(
                                        egui::pos2(cx + radius + gap * 0.5, cy),
                                        egui::vec2(radius * 2.0, radius * 2.0),
                                    );
                                    left_rect.contains(p) || right_rect.contains(p)
                                })
                                .unwrap_or(false)
                        } else {
                            false
                        };
                        if fs_response.clicked()
                            && !tile_active
                            && !in_hud
                            && !in_video_panel
                            && !in_center_buttons
                            && let Some(idx) = self.fullscreen_idx
                            && let Some(p) = self.fs_video_player(idx)
                        {
                            p.toggle_play();
                        }
                    } else if fs_response.clicked() {
                        // ポップアップ表示中はクリックでのページ送りを抑制
                        let any_popup = self.spread_popup_open;
                        if !any_popup {
                            if let Some(pos) = fs_response.interact_pointer_pos() {
                                let panel_threshold = full_rect.max.x - full_rect.width() * 0.25;
                                let in_right_panel = pos.y >= 60.0
                                    && (self.show_metadata_panel || pos.x > panel_threshold)
                                    && pos.x
                                        > full_rect.max.x
                                            - METADATA_PANEL_WIDTH.min(full_rect.width() * 0.5);
                                let in_left_panel = self.adjustment_mode
                                    && pos.x
                                        < full_rect.min.x
                                            + crate::ui_adjustment_panel::LEFT_PANEL_WIDTH
                                                .min(full_rect.width() * 0.3)
                                    && pos.y >= 60.0;
                                if !in_right_panel && !in_left_panel {
                                    let base = if pos.x > full_rect.center().x { 1 } else { -1 };
                                    nav_delta = self.spread_nav_delta(base, false);
                                }
                            }
                        }
                    }
                }
            }
        }
        // 抑制対象クリックの release フレームでここに来るので、左クリック分岐の
        // スキップが終わったこのタイミングでフラグを落とす。
        if suppress_this_frame && primary_released {
            self.fs_suppress_primary_until_release = false;
        }
        // 分析モード中は右クリックを色固定に使うため、終了トリガーにしない
        // コンテキストメニュー表示中は右クリック処理をスキップ
        if !self.analysis_mode && self.fs_context_menu_idx.is_none() {
            let secondary_down = ctx.input(|i| i.pointer.secondary_down());
            let secondary_released = ctx.input(|i| i.pointer.secondary_released());
            let secondary_pos = ctx.input(|i| i.pointer.interact_pos().unwrap_or_default());
            let secondary_in_video_hud = (secondary_down || secondary_released)
                && state.is_video
                && self.video_hud_visible_factor > 0.05
                && video_hud_rect(full_rect).contains(secondary_pos);

            if secondary_in_video_hud {
                if secondary_released {
                    self.fs_secondary_press_start = None;
                }
                return (nav_delta, close);
            }

            if secondary_down && self.fs_secondary_press_start.is_none() {
                // 押下開始を記録
                self.fs_secondary_press_start = Some((std::time::Instant::now(), secondary_pos));
            }

            if let Some((start_time, start_pos)) = self.fs_secondary_press_start {
                let elapsed = start_time.elapsed();
                let current_pos = ctx.input(|i| i.pointer.interact_pos().unwrap_or(start_pos));
                let moved = current_pos.distance(start_pos);

                if !secondary_released
                    && elapsed >= std::time::Duration::from_millis(400)
                    && moved < 20.0
                {
                    // 長押ししきい値超過 → 押下中にコンテキストメニューを即表示
                    self.fs_context_menu_idx = self.fullscreen_idx;
                    self.fs_context_menu_pos = current_pos;
                    self.fs_secondary_press_start = None;
                } else if secondary_released {
                    if moved < 20.0 && elapsed < std::time::Duration::from_millis(400) {
                        // 短押し → 従来通り閉じる
                        close = true;
                    }
                    self.fs_secondary_press_start = None;
                } else if moved >= 20.0 {
                    // マウスが動きすぎた → キャンセル
                    self.fs_secondary_press_start = None;
                }
            }
        }

        (nav_delta, close)
    }

    // ── ナビゲーション & スライドショー ─────────────────────────────────

    fn open_fullscreen_from_fs_navigation(&mut self, ctx: &egui::Context, idx: usize) {
        #[cfg(windows)]
        let restore_video_tile = self.video_tile_state.is_some();

        #[cfg(windows)]
        if restore_video_tile {
            self.video_tile_state = None;
            self.video_tile_swap_pending = None;
        }

        self.open_fullscreen(idx);

        #[cfg(windows)]
        {
            if restore_video_tile && matches!(self.items.get(idx), Some(GridItem::Video(_))) {
                self.video_tile_reopen_pending = true;
                self.video_tile_reopen_deadline =
                    Some(std::time::Instant::now() + std::time::Duration::from_secs(3));
                ctx.request_repaint();
            } else if restore_video_tile {
                self.video_tile_reopen_pending = false;
                self.video_tile_reopen_deadline = None;
            }
        }
    }

    /// フルスクリーン終了・ナビゲーション・スライドショーを処理する。
    fn handle_fs_navigation(
        &mut self,
        ctx: &egui::Context,
        close_fs: bool,
        ctrl_nav: Option<i32>,
        nav_delta: i32,
        jump_to: Option<usize>,
        fs_idx: usize,
    ) {
        if close_fs {
            self.close_fullscreen();
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        // Ctrl+↑↓ はフルスクリーンを保ったまま前後フォルダへ飛び、先頭/末尾の
        // 画像系アイテムを開く。self.selected も合わせて更新するので、ここから
        // フルスクリーンを閉じたときグリッド側のカーソルが最後に観た画像に残る。
        //
        // 実装上: `navigate_folder_with_skip` は DFS + `read_dir` で UI スレッドを
        // ブロックし得るので (深い階層だと 100ms 級)、ここでは発火だけ行い、
        // 実際の close_fullscreen / load_folder / open_fullscreen は
        // `apply_folder_nav_result` (FolderNavMode::Fullscreen ブランチ) に任せる。
        if let Some(delta) = ctrl_nav {
            // 連打時のロック: 直前の Ctrl+↓ で開始したナビの新ページ表示が
            // まだ準備できていない (= サムネ未ロード or fs_cache 未投入) 間は
            // 入力を捨てて、画面が「ファイル名だけ次々切り替わる」状態を防ぐ。
            // 待ちが解除されてから次のキーを押せば確実にサムネ以上が見える。
            if self.fs_nav_is_locked() {
                // ignore — 次フレームで poll_fs_nav_lock が解除する
            } else {
                let forward = delta > 0;
                // Ctrl+G 絞り込みビュー中はファイルシステム DFS ではなく検索結果の
                // NavEntry リスト上を移動する。fs ツリーを跨ぐと「検索結果の外」に
                // 出てしまうので、検索解除まで Ctrl+G スコープに閉じ込める。
                if self.global_search.active
                    && matches!(
                        self.global_search.view,
                        crate::global_search_ui::GlobalSearchView::DrilledInto { .. }
                    )
                {
                    self.global_search_ctrl_nav_fullscreen(forward);
                } else if let Some(cur) = self.current_folder.clone() {
                    // ナビ発火前に「今出ているテクスチャ」を holdover に退避し、
                    // items 入れ替えで fs_cache が drop されても画面が真っ白に
                    // ならないようにする。`capture_fs_nav_holdover` がロック取得時の
                    // items_generation も同時に記録する (= items 入れ替え前の早期解除を防ぐ)。
                    self.capture_fs_nav_holdover(fs_idx);
                    self.start_folder_nav(cur, forward, crate::app::FolderNavMode::Fullscreen);
                }
            }
        } else if !close_fs {
            if let Some(new_idx) = jump_to {
                self.open_fullscreen_from_fs_navigation(ctx, new_idx);
                self.selected = Some(new_idx);
                self.scroll_to_selected = true;
                self.update_last_selected_image();
            } else if nav_delta != 0 {
                if let Some(new_idx) = crate::ui_helpers::adjacent_navigable_idx(
                    &self.items,
                    &self.visible_indices,
                    fs_idx,
                    nav_delta,
                ) {
                    self.open_fullscreen_from_fs_navigation(ctx, new_idx);
                    self.selected = Some(new_idx);
                    self.scroll_to_selected = true;
                    self.update_last_selected_image();
                } else {
                    // 境界到達: 中央にヒントを出す (nav_delta > 0 なら末尾)
                    self.fs_boundary_hint = Some(FsBoundaryHint::Edge {
                        at_end: nav_delta > 0,
                        at: std::time::Instant::now(),
                    });
                    crate::logger::log(format!(
                        "[NAV] adjacent_navigable_idx returned None: fs_idx={fs_idx}, delta={nav_delta}, items={}, visible={}",
                        self.items.len(),
                        self.visible_indices.len()
                    ));
                }
            }
        }

        // ── スライドショー タイマー ──
        if self.slideshow_playing && !close_fs {
            let now = std::time::Instant::now();
            if now >= self.slideshow_next_at {
                if let Some(cur) = self.fullscreen_idx {
                    let slide_delta = self.spread_nav_delta(1, false);
                    let next = crate::ui_helpers::adjacent_navigable_idx(
                        &self.items,
                        &self.visible_indices,
                        cur,
                        slide_delta,
                    );
                    // 末尾到達時は先頭の画像系アイテムへループ。
                    // 画像系がひとつも無い場合はスライドショーを停止 (安全側、
                    // 旧実装の `unwrap_or(0)` で非画像アイテムへ飛ぶ事故を防ぐ)。
                    let target = next.or_else(|| {
                        self.visible_indices.iter().copied().find(|&i| {
                            matches!(
                                self.items.get(i),
                                Some(GridItem::Image(_))
                                    | Some(GridItem::ZipImage { .. })
                                    | Some(GridItem::PdfPage { .. })
                            )
                        })
                    });
                    match target {
                        Some(idx) => {
                            self.open_fullscreen_from_fs_navigation(ctx, idx);
                            self.selected = Some(idx);
                            self.scroll_to_selected = true;
                        }
                        None => {
                            self.slideshow_playing = false;
                        }
                    }
                }
                self.slideshow_next_at =
                    now + std::time::Duration::from_secs_f32(self.settings.slideshow_interval_secs);
            }
            let remaining = self.slideshow_next_at.saturating_duration_since(now);
            ctx.request_repaint_after(remaining);
        }
    }

    /// フルスクリーンの再描画リクエストを管理する。
    fn handle_fs_repaint(&self, ctx: &egui::Context, fs_idx: usize, is_video: bool) {
        // 高解像度読み込み完了まで、またはPDF再レンダリング中は毎フレーム再描画
        let image_loading = !is_video
            && self
                .fullscreen_idx
                .map(|i| !self.fs_cache.contains_key(&i))
                .unwrap_or(false);
        let pdf_rerendering = self.fs_pending.contains_key(&fs_idx);
        if image_loading || pdf_rerendering {
            ctx.request_repaint();
        }

        // 右クリック長押し検出中: しきい値チェックのため再描画をリクエスト
        if let Some((start_time, _)) = self.fs_secondary_press_start {
            let remaining =
                std::time::Duration::from_millis(400).saturating_sub(start_time.elapsed());
            if remaining.is_zero() {
                ctx.request_repaint();
            } else {
                ctx.request_repaint_after(remaining);
            }
        }

        // アニメーション: 次フレームの時刻まで待ってから再描画
        if !is_video {
            if let Some(FsCacheEntry::Animated { next_frame_at, .. }) = self.fs_cache.get(&fs_idx) {
                let delay = (next_frame_at - ctx.input(|i| i.time)).max(0.0);
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(delay));
            }
        }
    }

    // ── フルスクリーン描画ヘルパー ──────────────────────────────────────

    /// ZIP セパレータの章タイトル画面を描画する。
    fn draw_fs_separator(ui: &mut egui::Ui, full_rect: egui::Rect, sep: &str) {
        let title_size = (full_rect.height() * 0.12).clamp(48.0, 120.0);
        let sub_size = (full_rect.height() * 0.030).clamp(20.0, 36.0);

        ui.painter().rect_filled(
            egui::Rect::from_center_size(
                full_rect.center(),
                egui::vec2(full_rect.width() * 0.85, title_size * 2.2),
            ),
            16.0,
            egui::Color32::from_rgba_unmultiplied(30, 45, 80, 180),
        );
        ui.painter().text(
            full_rect.center(),
            egui::Align2::CENTER_CENTER,
            sep,
            egui::FontId::proportional(title_size),
            egui::Color32::WHITE,
        );
        ui.painter().text(
            egui::pos2(full_rect.center().x, full_rect.max.y - 48.0),
            egui::Align2::CENTER_BOTTOM,
            "── 作品の区切り ──",
            egui::FontId::proportional(sub_size),
            egui::Color32::from_rgb(150, 180, 220),
        );
    }

    /// フルスクリーンの画像 / 動画 / 読込中 / 失敗 表示を描画する。
    /// zoom/pan が Some のとき分析モードのズーム/パンを適用する。
    /// `bg_style` が Default 以外のとき、画像 rect の直下に透過背景を塗る。
    ///
    /// 動画は native presenter が独立 HWND に直接描画するので、ここでは
    /// 静止画 / アニメーション / サムネイル / プレースホルダーだけを扱う。
    #[allow(clippy::too_many_arguments)]
    fn draw_fs_image(
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
        tex: Option<&egui::TextureHandle>,
        thumb_tex: Option<&egui::TextureHandle>,
        is_video: bool,
        vst3_waiting_for_video: bool,
        fs_load_failed: bool,
        rotation: crate::rotation_db::Rotation,
        zoom_pan: Option<(f32, egui::Vec2)>,
        free_rotation_rad: f32,
        bg_style: &FsBgStyle<'_>,
        // 読込中プレースホルダ直下に出す対象パス (`location_display_for` 参照)。
        // 空ならラベル描画をスキップ。
        location_display: &str,
    ) {
        let display_tex = tex.or(thumb_tex);
        if let Some(handle) = display_tex {
            let tex_size = handle.size_vec2();
            let display_size = match rotation {
                crate::rotation_db::Rotation::Cw90 | crate::rotation_db::Rotation::Cw270 => {
                    egui::vec2(tex_size.y, tex_size.x)
                }
                _ => tex_size,
            };
            let fit_scale =
                (full_rect.width() / display_size.x).min(full_rect.height() / display_size.y);
            let (total_scale, center) = match zoom_pan {
                Some((zoom, pan)) => (fit_scale * zoom, full_rect.center() + pan),
                None => (fit_scale, full_rect.center()),
            };
            let img_rect = egui::Rect::from_center_size(center, display_size * total_scale);
            let needs_clip = zoom_pan.is_some() || free_rotation_rad.abs() > TRANSFORM_EPSILON;
            let painter = if needs_clip {
                ui.painter().with_clip_rect(full_rect)
            } else {
                ui.painter().clone()
            };
            // 透過画像用背景 (B キーで切替)。回転時は img_rect が回転前の bbox になるため
            // 視覚的ズレを避けて rotation が None のときのみ適用する。
            if rotation.is_none() && free_rotation_rad.abs() <= TRANSFORM_EPSILON {
                paint_transparent_bg(&painter, img_rect, bg_style);
            }
            if rotation.is_none() && free_rotation_rad.abs() <= TRANSFORM_EPSILON {
                painter.image(
                    handle.id(),
                    img_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                crate::app::draw_rotated_image_ex(
                    &painter,
                    handle.id(),
                    img_rect,
                    rotation,
                    free_rotation_rad,
                    center,
                );
            }
        } else if fs_load_failed {
            ui.painter().text(
                full_rect.center(),
                egui::Align2::CENTER_CENTER,
                "読込失敗",
                egui::FontId::proportional(32.0),
                egui::Color32::from_rgb(255, 140, 140),
            );
            ui.painter().text(
                full_rect.center() + egui::vec2(0.0, 40.0),
                egui::Align2::CENTER_CENTER,
                "このファイルはデコードできませんでした",
                egui::FontId::proportional(16.0),
                egui::Color32::from_gray(180),
            );
        } else {
            let painter = ui.painter();
            let loading_label = if vst3_waiting_for_video {
                "VST3 プラグインを初期化中..."
            } else if is_video {
                "動画サムネイル 読込中..."
            } else {
                "読込中..."
            };
            painter.text(
                full_rect.center(),
                egui::Align2::CENTER_CENTER,
                loading_label,
                egui::FontId::proportional(24.0),
                egui::Color32::from_gray(180),
            );
            crate::ui_helpers::draw_centered_elided_label(
                painter,
                full_rect,
                location_display,
                14.0,
                egui::Color32::from_gray(170),
                full_rect.center().y + 22.0,
                20.0,
            );
        }
    }

    /// 16×16 の市松テクスチャを lazy 生成する。
    /// B キーで「市松」モードになったとき初めて呼ばれる。
    pub(crate) fn ensure_checker_texture(&mut self, ctx: &egui::Context) {
        if self.fs_checker_texture.is_some() {
            return;
        }
        // 16×16 の 2 値グレー。タイルは 8×8 の 2 色。
        // Photoshop / GIMP 標準に近い中間グレーで、目に邪魔にならない配色。
        let mut rgba = Vec::with_capacity(16 * 16 * 4);
        for y in 0..16 {
            for x in 0..16 {
                let cell = ((x / 8) + (y / 8)) % 2;
                let v: u8 = if cell == 0 { 224 } else { 176 };
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let ci = egui::ColorImage::from_rgba_unmultiplied([16, 16], &rgba);
        let options = egui::TextureOptions {
            magnification: egui::TextureFilter::Nearest,
            minification: egui::TextureFilter::Nearest,
            wrap_mode: egui::TextureWrapMode::Repeat,
            mipmap_mode: None,
        };
        let tex = ctx.load_texture("fs_transparent_checker", ci, options);
        self.fs_checker_texture = Some(tex);
    }

    /// 現在の `fs_transparent_bg_mode` から描画スタイルを返す。
    /// 市松モードのときはテクスチャを lazy 生成する。
    fn fs_bg_style<'a>(&'a mut self, ctx: &egui::Context) -> FsBgStyle<'a> {
        if self.fs_transparent_bg_mode == 2 {
            self.ensure_checker_texture(ctx);
        }
        transparent_bg_style(
            self.fs_transparent_bg_mode,
            self.fs_checker_texture.as_ref(),
        )
    }

    /// 透過背景モードが Default 以外のとき、画面右上に現在モードを示す。
    /// モード変更直後 (`fs_transparent_bg_indicator_until` 有効) のみ表示。
    fn draw_fs_transparent_bg_indicator(&mut self, ui: &egui::Ui, full_rect: egui::Rect) {
        let Some(until) = self.fs_transparent_bg_indicator_until else {
            return;
        };
        let now = std::time::Instant::now();
        if now >= until {
            self.fs_transparent_bg_indicator_until = None;
            return;
        }
        // フェードアウト: 最後 400ms で alpha を下げる
        let remaining = until.saturating_duration_since(now);
        let alpha_f = (remaining.as_millis().min(400) as f32) / 400.0;
        let alpha = (alpha_f * 220.0) as u8;
        // 3 モード循環 (テーマ非依存): ビューポート既定の黒 (0) / 白 (1) / 市松 (2)。
        let label = match self.fs_transparent_bg_mode {
            1 => "背景: 白",
            2 => "背景: 市松",
            _ => "背景: 黒",
        };
        let painter = ui.painter();
        let font = egui::FontId::proportional(14.0);
        let galley = painter.layout_no_wrap(
            label.to_string(),
            font,
            egui::Color32::from_white_alpha(alpha),
        );
        let pos = egui::pos2(
            full_rect.max.x - galley.size().x - 16.0,
            full_rect.min.y + 12.0,
        );
        let bg = egui::Rect::from_min_size(pos, galley.size()).expand(6.0);
        painter.rect_filled(
            bg,
            4.0,
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, alpha.saturating_sub(40)),
        );
        painter.galley(pos, galley, egui::Color32::from_white_alpha(alpha));
        ui.ctx().request_repaint(); // フェードを継続
    }

    fn draw_original_preview_indicator(&self, ui: &egui::Ui, full_rect: egui::Rect, active: bool) {
        if !active {
            return;
        }
        let label = if cfg!(windows) {
            "元画像表示中: 右Ctrl"
        } else {
            "元画像表示中: 0"
        };
        let painter = ui.painter();
        let font = egui::FontId::proportional(14.0);
        let galley = painter.layout_no_wrap(label.to_string(), font, egui::Color32::WHITE);
        let pos = egui::pos2(full_rect.min.x + 16.0, full_rect.min.y + 12.0);
        let bg = egui::Rect::from_min_size(pos, galley.size()).expand(6.0);
        painter.rect_filled(bg, 4.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 190));
        painter.galley(pos, galley, egui::Color32::WHITE);
        ui.ctx().request_repaint();
    }

    /// ルーペ (局所拡大) 描画。
    ///
    /// 有効条件:
    /// - `fs_loupe_locked` が true (M キーでトグル) か、Shift キーホールド中
    /// - ビューポートにフォーカスがある
    /// - 分析モード・補正モードに入っていない
    /// - カーソルが `full_rect` 内
    /// - 画像が回転 0 / 任意回転なし (回転時は UV 逆変換が複雑なため v0.7.0 では非対応)
    /// - 現在 Single (非見開き) 表示で、テクスチャが存在する
    ///
    /// 見開き時は現状は未対応 (v0.7.0 のスコープ外)。
    pub(crate) fn draw_fs_loupe_if_active(
        &mut self,
        ui: &egui::Ui,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        fs_idx: usize,
        tex: Option<&egui::TextureHandle>,
        thumb_tex: Option<&egui::TextureHandle>,
        spread_double: bool,
    ) {
        if self.analysis_mode || self.adjustment_mode {
            return;
        }
        let (hover, shift_held, focused) = ctx.input(|i| {
            (
                i.pointer.hover_pos(),
                i.modifiers.shift,
                i.viewport().focused.unwrap_or(true),
            )
        });
        if !focused {
            return;
        }
        if !self.fs_loupe_locked && !shift_held {
            return;
        }
        let Some(cursor) = hover else { return };
        if !full_rect.contains(cursor) {
            return;
        }

        // ── 対象のページ矩形 + テクスチャを決定 ───────────────────────
        // Single / Spread で分岐。見開きはカーソル直下のページを選ぶ。
        let (img_rect, handle_owned, idx_for_rot) = if spread_double {
            let Some(layout) = self.fs_spread_layout else {
                return;
            };
            let (page_idx, page_rect) = if layout.left_rect.contains(cursor) {
                (layout.left_idx, layout.left_rect)
            } else if layout.right_rect.contains(cursor) {
                (layout.right_idx, layout.right_rect)
            } else {
                return;
            };
            // 見開き時はページ rect がそのまま image rect (draw_fs_spread が高さ統一で
            // アスペクトをぴったり合わせた矩形を組むため、leterbox は発生しない)。
            // テクスチャ取得 (fs_cache → thumbnail)
            let page_tex: Option<egui::TextureHandle> = match self.fs_cache.get(&page_idx) {
                Some(FsCacheEntry::Static { tex, .. }) => Some(tex.clone()),
                Some(FsCacheEntry::Animated {
                    frames,
                    current_frame,
                    ..
                }) => frames.get(*current_frame).map(|(h, _)| h.clone()),
                _ => None,
            };
            let page_thumb: Option<egui::TextureHandle> = match self.thumbnails.get(page_idx) {
                Some(ThumbnailState::Loaded { tex, .. }) => Some(tex.clone()),
                _ => None,
            };
            let Some(handle) = page_tex.or(page_thumb) else {
                return;
            };
            (page_rect, handle, page_idx)
        } else {
            let Some(handle) = tex.or(thumb_tex) else {
                return;
            };
            let tex_size = handle.size_vec2();
            if tex_size.x <= 0.0 || tex_size.y <= 0.0 {
                return;
            }
            let fit_scale = (full_rect.width() / tex_size.x).min(full_rect.height() / tex_size.y);
            let (total_scale, img_center) = match self.fs_zoom_pan() {
                Some((zoom, pan)) => (fit_scale * zoom, full_rect.center() + pan),
                None => (fit_scale, full_rect.center()),
            };
            let size = tex_size * total_scale;
            let rect = egui::Rect::from_center_size(img_center, size);
            (rect, handle.clone(), fs_idx)
        };

        // 回転 / 任意回転時は UV 逆変換が複雑なためルーペ非対応
        let rotation = self.get_rotation(idx_for_rot);
        if !rotation.is_none() || self.fs_free_rotation.abs() > TRANSFORM_EPSILON {
            return;
        }

        let tex_size = handle_owned.size_vec2();
        if tex_size.x <= 0.0 || tex_size.y <= 0.0 {
            return;
        }
        if !img_rect.contains(cursor) {
            return;
        }

        // 画面 px → テクスチャ px の変換倍率 (見開きでも単一でも共通のスカラー)
        let total_scale = img_rect.width() / tex_size.x;
        let uv_center = egui::vec2(
            (cursor.x - img_rect.min.x) / img_rect.width(),
            (cursor.y - img_rect.min.y) / img_rect.height(),
        );

        // ルーペパラメータ (将来設定化)
        const LOUPE_SIZE: f32 = 300.0;
        const LOUPE_ZOOM: f32 = 3.0;
        const LOUPE_OFFSET: f32 = 40.0;

        // サンプル UV: LOUPE_SIZE / LOUPE_ZOOM をテクスチャピクセル単位に変換
        let sample_px_half = LOUPE_SIZE * 0.5 / (total_scale * LOUPE_ZOOM);
        let half_uv = egui::vec2(sample_px_half / tex_size.x, sample_px_half / tex_size.y);
        let uv_min = egui::pos2(
            (uv_center.x - half_uv.x).clamp(0.0, 1.0),
            (uv_center.y - half_uv.y).clamp(0.0, 1.0),
        );
        let uv_max = egui::pos2(
            (uv_center.x + half_uv.x).clamp(0.0, 1.0),
            (uv_center.y + half_uv.y).clamp(0.0, 1.0),
        );
        let uv_rect = egui::Rect::from_min_max(uv_min, uv_max);

        // ポップアップ位置: カーソル右下 → はみ出すなら反転
        let mut popup_pos = cursor + egui::vec2(LOUPE_OFFSET, LOUPE_OFFSET);
        if popup_pos.x + LOUPE_SIZE > full_rect.max.x {
            popup_pos.x = cursor.x - LOUPE_OFFSET - LOUPE_SIZE;
        }
        if popup_pos.y + LOUPE_SIZE > full_rect.max.y {
            popup_pos.y = cursor.y - LOUPE_OFFSET - LOUPE_SIZE;
        }
        // それでも画面外に出る場合は full_rect の内側に寄せる
        popup_pos.x = popup_pos.x.max(full_rect.min.x + 4.0);
        popup_pos.y = popup_pos.y.max(full_rect.min.y + 4.0);
        let loupe_rect = egui::Rect::from_min_size(popup_pos, egui::vec2(LOUPE_SIZE, LOUPE_SIZE));

        let painter = ui.painter();
        // 背景 (黒で囲う) + 画像 + 枠線
        painter.rect_filled(loupe_rect.expand(3.0), 4.0, egui::Color32::BLACK);
        painter.image(handle_owned.id(), loupe_rect, uv_rect, egui::Color32::WHITE);
        painter.rect_stroke(
            loupe_rect,
            2.0,
            egui::Stroke::new(2.0, egui::Color32::WHITE),
            egui::StrokeKind::Outside,
        );
        // Shift ホールド中は再描画を継続 (キー離したら止める)
        if shift_held {
            ctx.request_repaint();
        }
    }

    /// 見開きモードの2ページ描画。
    /// 2枚の画像を隙間なく中央に配置し、境界に薄い黒線を描画する。
    fn draw_fs_spread(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        image_rect: egui::Rect,
        left_idx: usize,
        right_idx: usize,
        original_preview_active: bool,
    ) {
        let zoom_pan = self.fs_zoom_pan();
        let left_rot = self.get_rotation(left_idx);
        let right_rot = self.get_rotation(right_idx);
        // 各ページが読込中ブランチに落ちたときに出すパス。steady state では空文字列になり
        // `draw_centered_elided_label` が描画をスキップするので無駄な String 化を避ける。
        let left_location = self.location_display_for_loading(left_idx);
        let right_location = self.location_display_for_loading(right_idx);
        // 透過背景スタイル (bg_style はテクスチャ借用を含むため左右描画の前後で寿命に注意)
        // fs_bg_style は &mut self を要求するため先に解決してから以降は shared borrow に切り替える。
        // 透過画像が見開きの片方だけの場合もあるので両ページに同じ bg を適用する。
        let bg_tex = if self.fs_transparent_bg_mode == 2 {
            self.ensure_checker_texture(ctx);
            self.fs_checker_texture.clone()
        } else {
            None
        };
        let bg_style = transparent_bg_style(self.fs_transparent_bg_mode, bg_tex.as_ref());

        // 各ページの表示サイズを計算して、全体をフィットさせる
        // 片方だけフルサイズだとアスペクト比の微小差でレイアウトがジャンプするため、
        // 両方フルサイズが揃うまではサムネイルサイズに統一する
        let both_in_fs_cache =
            self.fs_cache.contains_key(&left_idx) && self.fs_cache.contains_key(&right_idx);
        let (left_size, right_size) = if both_in_fs_cache {
            (
                Self::get_display_size(left_idx, left_rot, &self.fs_cache, &self.thumbnails),
                Self::get_display_size(right_idx, right_rot, &self.fs_cache, &self.thumbnails),
            )
        } else {
            // サムネイルのみ使用（fs_cache を空マップとして渡す）
            let empty = std::collections::HashMap::new();
            (
                Self::get_display_size(left_idx, left_rot, &empty, &self.thumbnails),
                Self::get_display_size(right_idx, right_rot, &empty, &self.thumbnails),
            )
        };

        // ズーム/パンが有効な場合は image_rect でクリップする
        // (ズーム時にページが image_rect 外へはみ出して他の UI を覆わないようにするため)
        let painter = if zoom_pan.is_some() {
            ui.painter().with_clip_rect(image_rect)
        } else {
            ui.painter().clone()
        };

        if let (Some(ls), Some(rs)) = (left_size, right_size) {
            // 両ページの高さを揃える（高い方に合わせる）
            let combined_h = ls.y.max(rs.y);
            let left_w = ls.x * (combined_h / ls.y);
            let right_w = rs.x * (combined_h / rs.y);

            let combined_w = left_w + right_w;

            // 画面にフィットするスケール
            let fit_scale = (image_rect.width() / combined_w).min(image_rect.height() / combined_h);

            let (total_scale, center) = match zoom_pan {
                Some((zoom, pan)) => (fit_scale * zoom, image_rect.center() + pan),
                None => (fit_scale, image_rect.center()),
            };

            let scaled_lw = left_w * total_scale;
            let scaled_rw = right_w * total_scale;
            let scaled_h = combined_h * total_scale;

            // 全体を中央に配置
            let total_w = scaled_lw + scaled_rw;
            let start_x = center.x - total_w * 0.5;
            let start_y = center.y - scaled_h * 0.5;

            let left_rect = egui::Rect::from_min_size(
                egui::pos2(start_x, start_y),
                egui::vec2(scaled_lw, scaled_h),
            );
            let right_rect = egui::Rect::from_min_size(
                egui::pos2(start_x + scaled_lw, start_y),
                egui::vec2(scaled_rw, scaled_h),
            );

            // ナビ ロック中は旧ページのテクスチャを左右両方の最終フォールバックとして
            // 使い、「ファイル名のみ表示」状態を回避する。両ページに同じ holdover が
            // 出るのは束の間 (poll_fs_nav_lock がサムネ Loaded で解除する) なので許容。
            let holdover_for_locked = if self.fs_nav_is_locked() {
                self.fs_holdover_tex.clone()
            } else {
                None
            };
            let left_original_tex = original_preview_active
                .then(|| self.resolve_original_preview_tex(ctx, left_idx))
                .flatten();
            let right_original_tex = original_preview_active
                .then(|| self.resolve_original_preview_tex(ctx, right_idx))
                .flatten();
            for (rect, idx, rot, location, original_tex) in [
                (
                    left_rect,
                    left_idx,
                    left_rot,
                    &left_location,
                    left_original_tex.as_ref(),
                ),
                (
                    right_rect,
                    right_idx,
                    right_rot,
                    &right_location,
                    right_original_tex.as_ref(),
                ),
            ] {
                Self::draw_fs_spread_page(
                    &painter,
                    rect,
                    idx,
                    rot,
                    &self.adjustment_cache,
                    &self.fs_cache,
                    &self.thumbnails,
                    &bg_style,
                    location,
                    holdover_for_locked.as_ref(),
                    original_tex,
                );
            }

            // ルーペが参照するレイアウトを記録 (両ページのサイズが既知のときのみ信頼できる)
            self.fs_spread_layout = Some(FsSpreadLayout {
                left_idx,
                left_rect,
                right_idx,
                right_rect,
            });

            // 区切り線（2px 黒線）
            let divider_x = start_x + scaled_lw;
            painter.line_segment(
                [
                    egui::pos2(divider_x, start_y),
                    egui::pos2(divider_x, start_y + scaled_h),
                ],
                egui::Stroke::new(SPREAD_DIVIDER_WIDTH, egui::Color32::BLACK),
            );
        } else {
            // サイズ不明の場合は均等分割フォールバック
            // (ズーム/パンはサイズが分かってからでないと正しく計算できないため適用しない)
            let half_w = image_rect.width() / 2.0;
            let left_rect =
                egui::Rect::from_min_size(image_rect.min, egui::vec2(half_w, image_rect.height()));
            let right_rect = egui::Rect::from_min_size(
                egui::pos2(image_rect.min.x + half_w, image_rect.min.y),
                egui::vec2(half_w, image_rect.height()),
            );
            // フォールバック分岐でも nav ロック中の holdover を渡す (上のパス参照)。
            let holdover_for_locked = if self.fs_nav_is_locked() {
                self.fs_holdover_tex.clone()
            } else {
                None
            };
            let left_original_tex = original_preview_active
                .then(|| self.resolve_original_preview_tex(ctx, left_idx))
                .flatten();
            let right_original_tex = original_preview_active
                .then(|| self.resolve_original_preview_tex(ctx, right_idx))
                .flatten();
            for (rect, idx, rot, location, original_tex) in [
                (
                    left_rect,
                    left_idx,
                    left_rot,
                    &left_location,
                    left_original_tex.as_ref(),
                ),
                (
                    right_rect,
                    right_idx,
                    right_rot,
                    &right_location,
                    right_original_tex.as_ref(),
                ),
            ] {
                Self::draw_fs_spread_page(
                    &painter,
                    rect,
                    idx,
                    rot,
                    &self.adjustment_cache,
                    &self.fs_cache,
                    &self.thumbnails,
                    &bg_style,
                    location,
                    holdover_for_locked.as_ref(),
                    original_tex,
                );
            }
            // フォールバック分岐: サイズ未確定でアスペクト比が崩れる可能性があるため、
            // ルーペ用レイアウトには書かない (ルーペは非見開きパスのロジックで描画しない)。
            self.fs_spread_layout = None;
        }
    }

    /// テクスチャの表示サイズ（回転考慮）を返す。テクスチャ未取得なら None。
    fn get_display_size(
        idx: usize,
        rotation: crate::rotation_db::Rotation,
        fs_cache: &std::collections::HashMap<usize, FsCacheEntry>,
        thumbnails: &[ThumbnailState],
    ) -> Option<egui::Vec2> {
        let tex = match fs_cache.get(&idx) {
            Some(FsCacheEntry::Static { tex, .. }) => Some(tex.size_vec2()),
            Some(FsCacheEntry::Animated {
                frames,
                current_frame,
                ..
            }) => frames.get(*current_frame).map(|(h, _)| h.size_vec2()),
            _ => None,
        };
        let size = tex.or_else(|| {
            if let Some(ThumbnailState::Loaded { tex, .. }) = thumbnails.get(idx) {
                Some(tex.size_vec2())
            } else {
                None
            }
        })?;
        Some(match rotation {
            crate::rotation_db::Rotation::Cw90 | crate::rotation_db::Rotation::Cw270 => {
                egui::vec2(size.y, size.x)
            }
            _ => size,
        })
    }

    /// 見開きモードの1ページ分を指定領域に描画。
    /// `painter` は呼び出し側でクリップ済みのものを渡すことで、ズーム時のはみ出しを防ぐ。
    /// `location_display` は draw_fs_image と同じで、空なら読込中ラベル描画をスキップ。
    /// テクスチャ優先順位は adjustment_cache → fs_cache → thumbnail → holdover。
    #[allow(clippy::too_many_arguments)]
    fn draw_fs_spread_page(
        painter: &egui::Painter,
        rect: egui::Rect,
        idx: usize,
        rotation: crate::rotation_db::Rotation,
        adjustment_cache: &std::collections::HashMap<usize, FsCacheEntry>,
        fs_cache: &std::collections::HashMap<usize, FsCacheEntry>,
        thumbnails: &[ThumbnailState],
        bg_style: &FsBgStyle<'_>,
        location_display: &str,
        holdover_tex: Option<&egui::TextureHandle>,
        original_tex: Option<&egui::TextureHandle>,
    ) {
        // テクスチャ取得（補正済 or フルサイズ or サムネイル → ロック中なら最後に holdover）
        let tex = if let Some(tex) = original_tex {
            Some(tex.clone())
        } else {
            match adjustment_cache.get(&idx) {
                Some(FsCacheEntry::Static { tex, .. }) => Some(tex.clone()),
                _ => match fs_cache.get(&idx) {
                    Some(FsCacheEntry::Static { tex, .. }) => Some(tex.clone()),
                    Some(FsCacheEntry::Animated {
                        frames,
                        current_frame,
                        ..
                    }) => frames.get(*current_frame).map(|(h, _)| h.clone()),
                    _ => None,
                },
            }
        };
        let thumb_tex = match thumbnails.get(idx) {
            Some(ThumbnailState::Loaded { tex, .. }) => Some(tex.clone()),
            _ => None,
        };
        let display_tex = tex.as_ref().or(thumb_tex.as_ref()).or(holdover_tex);

        if let Some(handle) = display_tex {
            let tex_size = handle.size_vec2();
            let display_size = match rotation {
                crate::rotation_db::Rotation::Cw90 | crate::rotation_db::Rotation::Cw270 => {
                    egui::vec2(tex_size.y, tex_size.x)
                }
                _ => tex_size,
            };
            let fit_scale = (rect.width() / display_size.x).min(rect.height() / display_size.y);
            let img_rect = egui::Rect::from_center_size(rect.center(), display_size * fit_scale);
            // 回転中は bbox のズレを避けて背景を適用しない
            if rotation.is_none() {
                paint_transparent_bg(painter, img_rect, bg_style);
                painter.image(
                    handle.id(),
                    img_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                crate::app::draw_rotated_image(painter, handle.id(), img_rect, rotation);
            }
        } else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "読込中...",
                egui::FontId::proportional(18.0),
                egui::Color32::from_gray(150),
            );
            crate::ui_helpers::draw_centered_elided_label(
                painter,
                rect,
                location_display,
                12.0,
                egui::Color32::from_gray(150),
                rect.center().y + 18.0,
                12.0,
            );
        }
    }

    /// フルスクリーン UI が「クリーンな状態」(= 上部バー / 左右パネル / HUD / モーダルが
    /// 何も出ていない) か判定する。`true` かつアイドル時間が `CURSOR_HIDE_IDLE_SECS` を
    /// 超えたらマウスカーソルを `CursorIcon::None` で非表示にする。
    fn fs_ui_is_clean(&self, ctx: &egui::Context, full_rect: egui::Rect, is_video: bool) -> bool {
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let in_top = pointer.is_some_and(|p| p.y < TOP_BAR_HOVER_Y);
        let in_right = pointer.is_some_and(|p| p.x > full_rect.max.x - full_rect.width() * 0.25);
        let hud_visible = is_video && self.video_hud_visible_factor >= 0.05;
        !in_top
            && !in_right
            && !self.show_metadata_panel
            && !self.adjustment_mode
            && !self.erase_mode
            && !self.analysis_mode
            && !self.spread_popup_open
            && !self.video_speed_popup_open
            && self.fs_context_menu_idx.is_none()
            && !self.any_dialog_open()
            && !hud_visible
    }

    /// フルスクリーンのホバー時トップバーを描画する。
    #[allow(clippy::too_many_arguments)]
    /// 上部ホバーバーを描画する。`location_display` は左側に表示するパス文字列
    /// (`FsFrameState::location_display`)。通常は `<folder>\<filename>`、ZIP/PDF
    /// 内は `<archive-path> > <entry>`。
    fn draw_fs_hover_bar(
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        location_display: &str,
        image_dims: Option<(u32, u32)>,
        image_file_size: Option<u64>,
        // 原寸が GPU 上限超で表示が縮小版のとき true。dims のあとに⚠マーカーを出す。
        image_downscaled: bool,
        close_fs: &mut bool,
        nav_delta: &mut i32,
        show_info: &mut bool,
        force_show: bool,
        slideshow_playing: &mut bool,
        _slideshow_interval: &mut f32,
        rotate_cw: &mut bool,
        rotate_ccw: &mut bool,
        show_analysis: &mut bool,
        spread_mode: &mut SpreadMode,
        spread_popup_open: &mut bool,
        is_spread_double: bool,
        // AI アップスケール後のサイズとモデル名（表示用）。動画モードでは無視される。
        ai_upscale_info: Option<(&str, u32, u32)>,
        // 画像補正パネル表示トグル
        adjustment_mode: &mut bool,
        // 現在ページに個別補正が適用されているか (ボタン点灯用)
        has_page_override: bool,
        // PDF ページのコンテンツ種別 (非 PDF なら None)
        pdf_content_type: Option<PdfPageContentType>,
        // Phase 6: 動画モードか。true なら画像専用ボタンを隠し、▦ タイルボタンに
        // 切替、右側情報も動画情報に差し替える。
        is_video: bool,
        // 動画情報 (= info() の値抜粋、is_video=true のときのみ有効)。
        // (duration_secs, bit_rate_bps)
        video_meta: Option<(f64, i64)>,
        // ▦ タイルボタンの状態 + 押下フラグ。
        tile_active: bool,
        tile_pressed: &mut bool,
        // VST3 プラグイン管理ボタン (動画モード + vst3_enabled のときのみ表示)。
        // active = 管理パネルが既に開いている。pressed = クリックされた。
        show_vst3_button: bool,
        vst3_panel_open: bool,
        vst3_pressed: &mut bool,
    ) {
        let hover_in_top = ctx.input(|i| {
            i.pointer
                .hover_pos()
                .map(|p| p.y < TOP_BAR_HOVER_Y)
                .unwrap_or(false)
        });
        // adjustment_mode がオンならオーバーレイとして常に表示
        if !hover_in_top && !force_show && !*spread_popup_open && !*adjustment_mode {
            return;
        }

        let bar_rect =
            egui::Rect::from_min_size(full_rect.min, egui::vec2(full_rect.width(), TOP_BAR_HEIGHT));
        ui.painter().rect_filled(
            bar_rect,
            0.0,
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 200),
        );
        ui.painter().line_segment(
            [
                egui::pos2(bar_rect.min.x, bar_rect.max.y),
                egui::pos2(bar_rect.max.x, bar_rect.max.y),
            ],
            egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 60),
            ),
        );

        // ── ボタン群（右端から左に並べる）──
        let mut next_x = bar_rect.max.x - BAR_BUTTON_SIZE - BAR_BUTTON_MARGIN;

        // × 閉じるボタン
        let close_resp = draw_bar_button(
            ui,
            next_x,
            bar_rect.min.y + BAR_BUTTON_MARGIN,
            "fs_close_btn",
            |hovered| {
                if hovered {
                    egui::Color32::from_rgba_unmultiplied(220, 50, 50, 230)
                } else {
                    egui::Color32::from_rgba_unmultiplied(70, 70, 70, 200)
                }
            },
            false, // active 状態なし
            |p, c, r| draw_close_icon(p, c, r),
        );
        let close_resp = close_resp.on_hover_text("閉じる [Esc]");
        if close_resp.clicked() {
            *close_fs = true;
        }
        if close_resp.hovered() {
            *nav_delta = 0;
        }
        next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;

        // VST ボタン: 動画モード + VST3 機能 ON のときだけ表示。
        // クリックで管理パネルを開く / 閉じる。management panel は egui::Window で
        // フルスクリーンビューポート内に描画されるので動画の手前に出る。
        if show_vst3_button {
            let vst_resp = draw_bar_button(
                ui,
                next_x,
                bar_rect.min.y + BAR_BUTTON_MARGIN,
                "fs_vst3_btn",
                |hovered| bar_button_bg(hovered, vst3_panel_open),
                vst3_panel_open,
                |p, c, _r| draw_vst_text_label(p, c),
            );
            let vst_resp = vst_resp.on_hover_text(if vst3_panel_open {
                "VST3 プラグイン管理を閉じる"
            } else {
                "VST3 プラグイン管理を開く"
            });
            if vst_resp.clicked() {
                *vst3_pressed = true;
            }
            if vst_resp.hovered() {
                *nav_delta = 0;
            }
            next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;
        }

        // ▶/⏸ スライドショーボタン (画像モード) または ▦ タイルボタン (動画モード)
        if is_video {
            let tile_resp = draw_bar_button(
                ui,
                next_x,
                bar_rect.min.y + BAR_BUTTON_MARGIN,
                "fs_video_tile_btn",
                |hovered| bar_button_bg(hovered, tile_active),
                tile_active,
                |p, c, r| draw_tile_grid_icon(p, c, r),
            );
            let tile_resp = tile_resp.on_hover_text(if tile_active {
                "タイルモード解除 [S]"
            } else {
                "タイルモード [S]"
            });
            if tile_resp.clicked() {
                *tile_pressed = true;
            }
            if tile_resp.hovered() {
                *nav_delta = 0;
            }
        } else {
            let play_resp = draw_bar_button(
                ui,
                next_x,
                bar_rect.min.y + BAR_BUTTON_MARGIN,
                "fs_play_btn",
                |hovered| {
                    if *slideshow_playing {
                        egui::Color32::from_rgba_unmultiplied(60, 180, 60, 200)
                    } else if hovered {
                        egui::Color32::from_rgba_unmultiplied(100, 100, 100, 200)
                    } else {
                        egui::Color32::from_rgba_unmultiplied(70, 70, 70, 200)
                    }
                },
                false,
                |p, c, r| {
                    if *slideshow_playing {
                        draw_pause_icon(p, c, r);
                    } else {
                        draw_play_triangle(p, c, r);
                    }
                },
            );
            let play_resp = if *slideshow_playing {
                play_resp.on_hover_text("スライドショー停止")
            } else {
                play_resp.on_hover_text("スライドショー")
            };
            if play_resp.clicked() {
                *slideshow_playing = !*slideshow_playing;
            }
            if play_resp.hovered() {
                *nav_delta = 0;
            }
            next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;
        }

        // ↷ 右回転 / ↶ 左回転ボタン (画像のみ — 動画では意味を持たないため非表示)
        if !is_video {
            let rcw_resp = draw_bar_button(
                ui,
                next_x,
                bar_rect.min.y + BAR_BUTTON_MARGIN,
                "fs_rcw_btn",
                |hovered| bar_button_bg(hovered, false),
                false,
                |p, c, r| draw_rotate_icon(p, c, r, true),
            );
            let rcw_resp = rcw_resp.on_hover_text("右回転 [R]");
            if rcw_resp.clicked() {
                *rotate_cw = true;
            }
            if rcw_resp.hovered() {
                *nav_delta = 0;
            }
            next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;

            let rccw_resp = draw_bar_button(
                ui,
                next_x,
                bar_rect.min.y + BAR_BUTTON_MARGIN,
                "fs_rccw_btn",
                |hovered| bar_button_bg(hovered, false),
                false,
                |p, c, r| draw_rotate_icon(p, c, r, false),
            );
            let rccw_resp = rccw_resp.on_hover_text("左回転 [L]");
            if rccw_resp.clicked() {
                *rotate_ccw = true;
            }
            if rccw_resp.hovered() {
                *nav_delta = 0;
            }
            next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;
        }

        // ℹ Info ボタン
        let info_resp = draw_bar_button(
            ui,
            next_x,
            bar_rect.min.y + BAR_BUTTON_MARGIN,
            "fs_info_btn",
            |hovered| bar_button_bg(hovered, *show_info),
            *show_info,
            |p, c, r| draw_info_icon(p, c, r),
        );
        let info_resp = info_resp.on_hover_text("メタデータ [I / Tab]");
        if info_resp.clicked() {
            *show_info = !*show_info;
        }
        if info_resp.hovered() {
            *nav_delta = 0;
        }
        next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;

        // 🔬 分析ボタン（見開きダブル中は非表示。動画では意味を持たないため非表示）
        if !is_spread_double && !is_video {
            let analysis_resp = draw_bar_button(
                ui,
                next_x,
                bar_rect.min.y + BAR_BUTTON_MARGIN,
                "fs_analysis_btn",
                |hovered| bar_button_bg(hovered, *show_analysis),
                *show_analysis,
                |p, c, r| draw_analysis_icon(p, c, r),
            );
            let analysis_resp = analysis_resp.on_hover_text("分析ツール [Z]");
            if analysis_resp.clicked() {
                *show_analysis = !*show_analysis;
            }
            if analysis_resp.hovered() {
                *nav_delta = 0;
            }
            next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;
        }

        // 📖 見開きモードボタン (画像のみ。動画では非表示)
        let spread_active = spread_mode.is_spread();
        let sm = *spread_mode;
        let mut spread_resp_rect = egui::Rect::NOTHING;
        if !is_video {
            let spread_resp = draw_bar_button(
                ui,
                next_x,
                bar_rect.min.y + BAR_BUTTON_MARGIN,
                "fs_spread_btn",
                |hovered| bar_button_bg(hovered, spread_active),
                spread_active,
                |p, c, r| draw_spread_icon(p, c, r, sm),
            );
            let spread_resp = spread_resp.on_hover_text("見開き設定 [1-5]");
            spread_resp_rect = spread_resp.rect;
            if spread_resp.clicked() {
                *spread_popup_open = !*spread_popup_open;
            }
            if spread_resp.hovered() {
                *nav_delta = 0;
            }
        } else if *spread_popup_open {
            // 動画モードに切り替わったときは popup を閉じる (見開きは画像のみ)
            *spread_popup_open = false;
        }

        // 見開きポップアップ (画像のみ)
        if *spread_popup_open && !is_video {
            let popup_x = next_x;
            let popup_y = bar_rect.max.y + 4.0;
            let popup_w = 200.0_f32;
            let popup_h = 5.0 * 36.0 + 8.0; // 5 items + padding
            let popup_rect = egui::Rect::from_min_size(
                egui::pos2(popup_x, popup_y),
                egui::vec2(popup_w, popup_h),
            );

            // 背景
            ui.painter().rect_filled(
                popup_rect,
                6.0,
                egui::Color32::from_rgba_unmultiplied(30, 30, 30, 240),
            );
            ui.painter().rect_stroke(
                popup_rect,
                6.0,
                egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(100, 100, 100, 180),
                ),
                egui::StrokeKind::Outside,
            );

            let mut item_y = popup_rect.min.y + 4.0;
            for &mode in SpreadMode::all() {
                let item_rect = egui::Rect::from_min_size(
                    egui::pos2(popup_rect.min.x + 4.0, item_y),
                    egui::vec2(popup_w - 8.0, 32.0),
                );
                let item_resp = ui.interact(
                    item_rect,
                    egui::Id::new(format!("spread_popup_{}", mode.to_int())),
                    egui::Sense::click(),
                );
                let is_current = *spread_mode == mode;
                let bg = if is_current {
                    egui::Color32::from_rgba_unmultiplied(80, 140, 220, 200)
                } else if item_resp.hovered() {
                    egui::Color32::from_rgba_unmultiplied(80, 80, 80, 200)
                } else {
                    egui::Color32::TRANSPARENT
                };
                ui.painter().rect_filled(item_rect, 4.0, bg);

                // アイコン (左側)
                let icon_center = egui::pos2(item_rect.min.x + 20.0, item_rect.center().y);
                draw_spread_icon(ui.painter(), icon_center, 7.0, mode);

                // ラベル (右側)
                ui.painter().text(
                    egui::pos2(item_rect.min.x + 44.0, item_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    mode.label(),
                    egui::FontId::proportional(13.0),
                    egui::Color32::from_gray(220),
                );

                let shortcut_label = match mode.to_int() {
                    0 => "[5]",
                    1 => "[6]",
                    2 => "[7]",
                    3 => "[8]",
                    _ => "[9]",
                };
                ui.painter().text(
                    egui::pos2(item_rect.max.x - 8.0, item_rect.center().y),
                    egui::Align2::RIGHT_CENTER,
                    shortcut_label,
                    egui::FontId::proportional(11.0),
                    egui::Color32::from_gray(140),
                );

                if item_resp.clicked() {
                    *spread_mode = mode;
                    *spread_popup_open = false;
                }
                item_y += 36.0;
            }

            // ポップアップ外クリックで閉じる
            let pointer_pos = ctx.input(|i| i.pointer.press_origin());
            if let Some(pos) = pointer_pos {
                if !popup_rect.contains(pos) && !spread_resp_rect.contains(pos) {
                    if ctx.input(|i| i.pointer.any_pressed()) {
                        *spread_popup_open = false;
                    }
                }
            }
        }

        if !is_video {
            next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;
        }

        // 🎨 画像補正パネルトグルボタン (動画では非表示)
        if !is_video {
            let btn_rect = egui::Rect::from_min_size(
                egui::pos2(next_x, bar_rect.min.y + BAR_BUTTON_MARGIN),
                egui::vec2(BAR_BUTTON_SIZE, BAR_BUTTON_SIZE),
            );
            let resp = ui.interact(
                btn_rect,
                egui::Id::new("fs_adjust_btn"),
                egui::Sense::click(),
            );
            let bg = if *adjustment_mode {
                egui::Color32::from_rgba_unmultiplied(80, 140, 220, 220)
            } else if has_page_override {
                // 個別設定が効いているときは薄い警告色でヒント
                egui::Color32::from_rgba_unmultiplied(120, 100, 60, 200)
            } else if resp.hovered() {
                egui::Color32::from_rgba_unmultiplied(100, 100, 100, 200)
            } else {
                egui::Color32::from_rgba_unmultiplied(70, 70, 70, 200)
            };
            ui.painter().rect_filled(btn_rect, 4.0, bg);
            ui.painter().text(
                btn_rect.center(),
                egui::Align2::CENTER_CENTER,
                "🎨",
                egui::FontId::proportional(16.0),
                egui::Color32::WHITE,
            );
            let tooltip = if has_page_override {
                "画像補正 (このページは個別設定あり)"
            } else {
                "画像補正"
            };
            let resp = resp.on_hover_text(tooltip);
            if resp.clicked() {
                *adjustment_mode = !*adjustment_mode;
            }
            if resp.hovered() {
                *nav_delta = 0;
            }
            next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;
        }

        // ── 左側: フォルダ + ファイル名 (または archive > entry) ──
        // 右側のボタン / 情報テキストと衝突しないように幅制限して右端を切る。
        // Phase 6: 動画モードでは AI アップスケール / PDF 情報を渡さず、
        // 動画専用の info text に切り替える。
        let info_text = if is_video {
            build_info_text_video(image_dims, image_file_size, video_meta)
        } else {
            build_info_text(
                image_dims,
                image_file_size,
                image_downscaled,
                ai_upscale_info,
                pdf_content_type,
            )
        };
        if !location_display.is_empty() {
            let info_w = if info_text.is_empty() {
                0.0
            } else {
                ui.painter()
                    .layout_no_wrap(
                        info_text.clone(),
                        egui::FontId::proportional(15.0),
                        egui::Color32::WHITE,
                    )
                    .size()
                    .x
            };
            let max_x = next_x - 12.0 - info_w;
            let avail_width = (max_x - (bar_rect.min.x + 12.0)).max(40.0);
            let galley = ui.painter().layout(
                location_display.to_string(),
                egui::FontId::proportional(13.0),
                egui::Color32::from_gray(200),
                avail_width,
            );
            let text_y = bar_rect.center().y - galley.size().y * 0.5;
            ui.painter().galley(
                egui::pos2(bar_rect.min.x + 12.0, text_y),
                galley,
                egui::Color32::from_gray(200),
            );
        }

        // ── 右側: 画像サイズ / 動画情報 / ファイルサイズ ──
        if is_video {
            if !info_text.is_empty() {
                ui.painter().text(
                    egui::pos2(next_x - 12.0, bar_rect.center().y),
                    egui::Align2::RIGHT_CENTER,
                    info_text,
                    egui::FontId::proportional(15.0),
                    egui::Color32::WHITE,
                );
            }
        } else {
            draw_fs_bar_info_text(
                ui,
                bar_rect,
                egui::pos2(next_x - 12.0, bar_rect.center().y),
                image_dims,
                image_file_size,
                image_downscaled,
                ai_upscale_info,
                pdf_content_type,
            );
        }
    }
}

// ── フルスクリーン AI ステータスオーバーレイ ────────────────────────────

impl App {
    /// 現在有効な AI 処理のモデル名を結合して返す。
    /// `show_auto_prefix` が true の場合、自動選択時に「自動: 」プレフィックスを付ける。
    fn ai_model_label(&self, fs_idx: usize, show_auto_prefix: bool) -> String {
        let mut labels = Vec::new();
        if let Some(denoise_kind) = self.ai_denoise_model {
            labels.push(denoise_kind.display_label().to_string());
        }
        if self.ai_upscale_enabled {
            let upscale_label = match self.ai_upscale_model_override {
                Some(k) => k.display_label().to_string(),
                None => self
                    .ai_classify_cache
                    .get(&fs_idx)
                    .map(|c| {
                        if show_auto_prefix {
                            format!("自動: {}", c.display_label())
                        } else {
                            c.display_label().to_string()
                        }
                    })
                    .unwrap_or_else(|| "自動".to_string()),
            };
            labels.push(upscale_label);
        }
        labels.join(" + ")
    }

    /// フルスクリーン左下に AI 処理ステータスを表示する。
    fn draw_fs_ai_status(&mut self, ui: &mut egui::Ui, fs_idx: usize) {
        let bg = self.effective_upscale_bg_mode();
        let is_upscaling = self.ai_upscale_pending.contains_key(&(fs_idx, bg));
        let is_upscaled = self.ai_upscale_cache.contains_key(&(fs_idx, bg));
        let is_loading = self.fs_pending.contains_key(&fs_idx);
        let any_busy = is_loading || is_upscaling || !self.ai_upscale_pending.is_empty();

        let mut lines: Vec<(String, egui::Color32)> = Vec::new();

        if is_loading {
            lines.push(("読込中...".to_string(), egui::Color32::from_gray(210)));
        }

        if is_upscaling {
            let label = self.ai_model_label(fs_idx, true);
            lines.push((
                format!("AI 処理中 ({})", label),
                egui::Color32::from_rgb(255, 200, 80),
            ));
        } else if is_upscaled {
            let label = self.ai_model_label(fs_idx, false);
            lines.push((
                format!("AI 処理完了 ({})", label),
                egui::Color32::from_rgb(80, 220, 80),
            ));
        }

        if self.erase_base_cache.contains_key(&fs_idx) && !self.erase_mode {
            lines.push((
                "消去補完済み".to_string(),
                egui::Color32::from_rgb(180, 140, 255),
            ));
        }

        // AI 機能が完全に無効なら先読みバーを出さない。
        // 以前は AI off でも target があれば「0/N」バーが表示されて
        // 進捗が進まないように見える UX 不具合があった。
        let ai_feature_active = self.ai_upscale_enabled || self.ai_denoise_model.is_some();

        let prefetch_progress: Option<(usize, usize)> = if is_upscaling || !ai_feature_active {
            None
        } else {
            let targets = self.ai_prefetch_targets(fs_idx);
            let total = targets.len();
            if total == 0 {
                None
            } else {
                // 「done」の判定: cache 済み / failed / サイズ閾値で skip 確定。
                // 高解像度スキャン (2048px 超等) は maybe_start_ai_upscale で
                // should_process に弾かれて AI が走らないが、従来は cache にも
                // failed にも入らないため「0/N」バーが永久に残った。
                // ここでサイズを見て「この画像は AI 対象外」と判別できるものは done
                // 扱いにする。fs_cache に Static が無いものはまだ判定不能なので undone。
                let upscale_px = self.settings.ai_upscale_skip_px;
                let denoise_px = self.settings.ai_denoise_skip_px;
                let upscale_enabled = self.ai_upscale_enabled;
                let denoise_enabled = self.ai_denoise_model.is_some();
                let done = targets
                    .iter()
                    .filter(|&&i| {
                        if self.ai_upscale_cache.contains_key(&(i, bg))
                            || self.ai_upscale_failed.contains(&(i, bg))
                        {
                            return true;
                        }
                        // fs_cache の dims でサイズ閾値判定
                        if let Some(FsCacheEntry::Static { pixels, .. }) = self.fs_cache.get(&i) {
                            let w = pixels.size[0] as u32;
                            let h = pixels.size[1] as u32;
                            let upscale_skip = !upscale_enabled
                                || !crate::ai::upscale::should_process(w, h, upscale_px);
                            let denoise_skip = !denoise_enabled
                                || !crate::ai::upscale::should_process(w, h, denoise_px);
                            if upscale_skip && denoise_skip {
                                return true;
                            }
                        }
                        false
                    })
                    .count();
                (done < total).then_some((done, total))
            }
        };

        if lines.is_empty() && prefetch_progress.is_none() {
            self.ai_status_done_at = None;
            return;
        }

        // 全処理完了後の自動非表示: 完了から 1 秒フル表示、続く 1 秒でフェードアウト。
        const FADE_START_SECS: f32 = 1.0;
        const FADE_DURATION_SECS: f32 = 1.0;
        if any_busy {
            self.ai_status_done_at = None;
        } else {
            let done_at = *self
                .ai_status_done_at
                .get_or_insert_with(std::time::Instant::now);
            if done_at.elapsed().as_secs_f32() > FADE_START_SECS + FADE_DURATION_SECS {
                return;
            }
        }

        let alpha = if let Some(done_at) = self.ai_status_done_at {
            let elapsed = done_at.elapsed().as_secs_f32();
            if elapsed < FADE_START_SECS {
                1.0
            } else {
                (1.0 - (elapsed - FADE_START_SECS) / FADE_DURATION_SECS).clamp(0.0, 1.0)
            }
        } else {
            1.0
        };

        // Area の available width が 0 のまま描画されるとラベルが 1 文字幅で
        // 縦に折り返される。min_width で横方向を確保する。
        const MIN_WIDTH: f32 = 260.0;
        const BAR_WIDTH: f32 = 180.0;
        const FONT_SIZE: f32 = 13.0;

        let ctx = ui.ctx().clone();
        egui::Area::new("fs_ai_status_overlay".into())
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(12.0, -12.0))
            .show(&ctx, |ui| {
                ui.set_opacity(alpha);
                ui.set_min_width(MIN_WIDTH);
                egui::Frame::popup(ui.style())
                    .fill(crate::ui_helpers::PROGRESS_BG_COLOR)
                    .show(ui, |ui| {
                        for (text, color) in &lines {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(text).color(*color).size(FONT_SIZE),
                                )
                                .wrap_mode(egui::TextWrapMode::Extend),
                            );
                        }
                        if let Some((done, total)) = prefetch_progress {
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new("先読み AI")
                                            .monospace()
                                            .color(crate::ui_helpers::PROGRESS_LABEL_COLOR),
                                    )
                                    .wrap_mode(egui::TextWrapMode::Extend),
                                );
                                ui.add(
                                    egui::ProgressBar::new(done as f32 / total as f32)
                                        .desired_width(BAR_WIDTH)
                                        .fill(crate::ui_helpers::PROGRESS_UPGRADE_COLOR)
                                        .text(
                                            egui::RichText::new(format!("{} / {}", done, total))
                                                .color(egui::Color32::BLACK),
                                        ),
                                );
                            });
                        }
                    });
            });

        // フェードアウト中のみ毎フレーム再描画。処理中の進捗更新は
        // poll_ai_upscale / poll_prefetch 側が完了時に repaint を要求するので
        // ここでの busy-loop repaint は不要。
        if self.ai_status_done_at.is_some() {
            ctx.request_repaint();
        }
    }
}

impl App {
    /// 右上にフィードバックトーストを描画する。
    /// フルスクリーン側 (`render_fullscreen_viewport`) とグリッド側 (`render_grid`) の
    /// 両方から呼ばれる。どちらで描画しても同じ見た目になるよう、描画先 `ui` と
    /// トーストの基準矩形 `full_rect` を呼び出し側が渡す。
    pub(crate) fn draw_feedback_toast(
        &mut self,
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
        ctx: &egui::Context,
    ) {
        let Some((ref text, start_time)) = self.fs_feedback_toast else {
            return;
        };
        let elapsed = start_time.elapsed().as_secs_f32();
        if elapsed > FEEDBACK_TOAST_DURATION {
            self.fs_feedback_toast = None;
            return;
        }

        // フェードアウト (最後の0.3秒)
        let alpha = if elapsed > FEEDBACK_TOAST_DURATION - 0.3 {
            ((FEEDBACK_TOAST_DURATION - elapsed) / 0.3).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let alpha_u8 = (alpha * 220.0) as u8;

        let font = egui::FontId::proportional(18.0);
        let galley = ui
            .painter()
            .layout_no_wrap(text.clone(), font.clone(), egui::Color32::WHITE);
        let text_size = galley.size();
        let padding = egui::vec2(16.0, 10.0);
        let toast_size = text_size + padding * 2.0;

        let toast_rect = egui::Rect::from_min_size(
            egui::pos2(
                full_rect.max.x - toast_size.x - 20.0,
                full_rect.min.y + 60.0,
            ),
            toast_size,
        );

        ui.painter().rect_filled(
            toast_rect,
            8.0,
            egui::Color32::from_rgba_unmultiplied(30, 30, 30, alpha_u8),
        );
        ui.painter().text(
            toast_rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            font,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, (alpha * 255.0) as u8),
        );

        // フェードアウト中は 30fps で再描画
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }

    /// 画面中央に境界ヒント (最初/最後の画像です… / 次のフォルダが見つかりません…) を描画する。
    fn draw_boundary_hint(
        &mut self,
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
        ctx: &egui::Context,
    ) {
        let Some(hint) = self.fs_boundary_hint else {
            return;
        };
        let start_time = hint.started_at();
        let duration = match hint {
            FsBoundaryHint::Edge { .. } => BOUNDARY_HINT_DURATION,
            FsBoundaryHint::NoImageFolder { .. } => NO_IMAGE_FOLDER_HINT_DURATION,
            FsBoundaryHint::SearchEnd { .. } => NO_IMAGE_FOLDER_HINT_DURATION,
        };
        let elapsed = start_time.elapsed().as_secs_f32();
        if elapsed > duration {
            self.fs_boundary_hint = None;
            return;
        }

        let alpha = if elapsed > duration - 0.4 {
            ((duration - elapsed) / 0.4).clamp(0.0, 1.0)
        } else {
            1.0
        };

        let (title, body_lines): (&str, Vec<&str>) = match hint {
            FsBoundaryHint::Edge { at_end: true, .. } => (
                "最後の画像です",
                vec!["[Home] 最初に戻る", "[Ctrl]+[↓] 次のフォルダへ"],
            ),
            FsBoundaryHint::Edge { at_end: false, .. } => (
                "最初の画像です",
                vec!["[End] 最後に移動", "[Ctrl]+[↑] 前のフォルダへ"],
            ),
            FsBoundaryHint::NoImageFolder { forward: true, .. } => (
                "次のフォルダに画像が見つかりません",
                vec![
                    "[Esc] でサムネイル一覧に戻り",
                    "[Ctrl]+[↓] で空フォルダを越えて移動できます",
                ],
            ),
            FsBoundaryHint::NoImageFolder { forward: false, .. } => (
                "前のフォルダに画像が見つかりません",
                vec![
                    "[Esc] でサムネイル一覧に戻り",
                    "[Ctrl]+[↑] で空フォルダを越えて移動できます",
                ],
            ),
            FsBoundaryHint::SearchEnd { forward: true, .. } => (
                "最後の検索結果です",
                vec!["[Esc] で検索を閉じると", "通常のフォルダ移動に戻ります"],
            ),
            FsBoundaryHint::SearchEnd { forward: false, .. } => (
                "最初の検索結果です",
                vec!["[Esc] で検索を閉じると", "通常のフォルダ移動に戻ります"],
            ),
        };

        let title_font = egui::FontId::proportional(32.0);
        let body_font = egui::FontId::proportional(22.0);
        let white = egui::Color32::from_rgba_unmultiplied(255, 255, 255, (alpha * 255.0) as u8);
        let accent = egui::Color32::from_rgba_unmultiplied(255, 220, 120, (alpha * 255.0) as u8);

        let painter = ui.painter();
        let title_galley = painter.layout_no_wrap(title.to_string(), title_font.clone(), white);
        let body_galleys: Vec<_> = body_lines
            .iter()
            .map(|s| painter.layout_no_wrap(s.to_string(), body_font.clone(), white))
            .collect();

        let line_gap = 10.0;
        let padding = egui::vec2(32.0, 24.0);
        let content_w = body_galleys
            .iter()
            .map(|g| g.size().x)
            .fold(title_galley.size().x, f32::max);
        let body_h: f32 = body_galleys.iter().map(|g| g.size().y).sum::<f32>()
            + line_gap * (body_galleys.len().saturating_sub(1) as f32);
        let content_h = title_galley.size().y + line_gap * 1.5 + body_h;
        let box_size = egui::vec2(content_w, content_h) + padding * 2.0;

        let center = full_rect.center();
        let box_rect = egui::Rect::from_center_size(center, box_size);

        let bg_alpha = (alpha * 210.0) as u8;
        painter.rect_filled(
            box_rect,
            12.0,
            egui::Color32::from_rgba_unmultiplied(20, 20, 20, bg_alpha),
        );
        painter.rect_stroke(
            box_rect,
            12.0,
            egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(200, 200, 200, (alpha * 120.0) as u8),
            ),
            egui::StrokeKind::Outside,
        );

        let mut y = box_rect.min.y + padding.y;
        painter.text(
            egui::pos2(center.x, y),
            egui::Align2::CENTER_TOP,
            title,
            title_font,
            accent,
        );
        y += title_galley.size().y + line_gap * 1.5;
        for (line, galley) in body_lines.iter().zip(body_galleys.iter()) {
            painter.text(
                egui::pos2(center.x, y),
                egui::Align2::CENTER_TOP,
                *line,
                body_font.clone(),
                white,
            );
            y += galley.size().y + line_gap;
        }

        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
}

// ===========================================================================
// 動画インライン再生用 UI ヘルパー
// ===========================================================================

impl App {
    /// シーク先サムネを GPU テクスチャに反映 (in-place set。bucket key 単位で更新)。
    /// 戻り値は描画に使う `egui::TextureId`。
    fn upload_seek_thumb_texture(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        thumb: &crate::video::thumbnail::Thumbnail,
    ) -> egui::TextureId {
        // bucket key: thumbnail::SECONDS_PER_BUCKET (= 0.5s) の整数。同 key なら upload skip。
        let key = crate::video::thumbnail::bucket_key(thumb.target_secs);
        let need_recreate = match &self.video_seek_thumb_tex {
            Some((idx, _, _)) if *idx != fs_idx => true,
            None => true,
            _ => false,
        };
        let need_set = match &self.video_seek_thumb_tex {
            Some((_, k, _)) if *k == key && !need_recreate => false,
            _ => true,
        };
        let color = egui::ColorImage::from_rgba_unmultiplied(
            [thumb.width as usize, thumb.height as usize],
            &thumb.rgba,
        );
        if need_recreate {
            let label = format!("video_seek_thumb:{fs_idx}");
            let tex = ctx.load_texture(label, color, egui::TextureOptions::LINEAR);
            let id = tex.id();
            self.video_seek_thumb_tex = Some((fs_idx, key, tex));
            id
        } else {
            if let Some((_, k, tex)) = self.video_seek_thumb_tex.as_mut() {
                if need_set {
                    tex.set(color, egui::TextureOptions::LINEAR);
                    *k = key;
                }
                tex.id()
            } else {
                // 到達不可だが安全な fallback
                let label = format!("video_seek_thumb:{fs_idx}");
                let tex = ctx.load_texture(label, color, egui::TextureOptions::LINEAR);
                let id = tex.id();
                self.video_seek_thumb_tex = Some((fs_idx, key, tex));
                id
            }
        }
    }

    /// 現在 `fs_idx` のキャッシュエントリが動画なら `&VideoPlayer` を返す。
    /// HUD 各ウィジェットの click handler / 上位の toggle_play で使う。
    pub(crate) fn fs_video_player(&self, fs_idx: usize) -> Option<&crate::video::VideoPlayer> {
        match self.fs_cache.get(&fs_idx)? {
            FsCacheEntry::Video { player, .. } => Some(player),
            _ => None,
        }
    }

    pub(crate) fn step_video_frame(&mut self, ctx: &egui::Context, fs_idx: usize, direction: i32) {
        if let Some(player) = self.fs_video_player(fs_idx) {
            player.step_frame(direction);
            self.video_hud_last_activity = Some(std::time::Instant::now());
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    pub(crate) fn copy_video_frame_to_clipboard(&mut self, fs_idx: usize) {
        let Some((path, target_secs)) = self.fs_video_player(fs_idx).and_then(|player| {
            if player.error().is_some() || player.info().is_none() {
                None
            } else {
                Some((player.path().clone(), player.screenshot_target_secs()))
            }
        }) else {
            self.show_feedback_toast("動画フレームをコピーできません".to_string());
            return;
        };

        self.show_feedback_toast("動画フレームをクリップボードへコピー中".to_string());
        let clipboard_seq = crate::ui_dialogs::context_menu::reserve_clipboard_write_sequence();
        std::thread::Builder::new()
            .name("video-frame-copy".into())
            .spawn(move || match crate::video::screenshot::capture_frame(&path, target_secs) {
                Ok(frame) => {
                    crate::logger::log(format!(
                        "video frame copied to clipboard: target={:.3}s decoded_target={:.3}s {}x{} {}",
                        target_secs,
                        frame.target_secs,
                        frame.width,
                        frame.height,
                        path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                    ));
                    crate::ui_dialogs::context_menu::copy_rgba_image_to_clipboard_async_seq(
                        frame.width,
                        frame.height,
                        frame.rgba,
                        clipboard_seq,
                    );
                }
                Err(err) => {
                    crate::logger::log(format!(
                        "video frame copy failed: target={target_secs:.3}s {}: {err}",
                        path.display()
                    ));
                }
            })
            .ok();
    }

    /// 動画のチャプター開始 / ブックマーク / ピンを 1 本の Vec に集約し pts 昇順で返す。
    /// シークバー描画 (マーカー縦線) と J/K ジャンプの両方が同じソースを使う。
    pub(crate) fn collect_video_nav_markers(&mut self, fs_idx: usize) -> Vec<NavMarker> {
        self.ensure_fullscreen_video_marker_cache(fs_idx);
        let path = match self.fs_video_player(fs_idx) {
            Some(p) => p.path().clone(),
            None => return Vec::new(),
        };
        let mut markers: Vec<NavMarker> = Vec::new();
        if let Some(info) = self.fs_video_player(fs_idx).and_then(|p| p.info()) {
            for c in &info.chapters {
                markers.push(NavMarker {
                    pts: c.start_secs,
                    kind: NavMarkerKind::Chapter,
                    title: c.title.clone(),
                });
            }
        }
        let (pin_pts, bookmarks) = self.fullscreen_video_marker_snapshot(fs_idx, &path);
        for bookmark in bookmarks {
            markers.push(NavMarker {
                pts: bookmark.pts_secs,
                kind: NavMarkerKind::Bookmark,
                title: bookmark.title,
            });
        }
        if let Some(pts) = pin_pts {
            markers.push(NavMarker {
                pts,
                kind: NavMarkerKind::Pin,
                title: None,
            });
        }
        markers.sort_by(|a, b| {
            a.pts
                .partial_cmp(&b.pts)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        markers
    }

    /// 動画再生時のキー入力を処理する。フルスクリーン中で `state.is_video` のときに
    /// 1 フレームに 1 回呼ぶ。
    pub(crate) fn handle_video_input(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        video_path: Option<&std::path::Path>,
    ) {
        // IME 変換中はショートカットを発火させない
        if self.ime_input_active() {
            return;
        }

        // 動画モードのキー処理: Space は consume せず後段の image 選択トグルに流す
        // (Phase 5.1: 画像と動画でキーアサインを揃える)。
        // 再生/一時停止トグルは **Enter** に移行。Shift+Enter は外部プレイヤー起動。
        // egui の `consume_key` は修飾子マッチが厳密 (Caps Lock + Shift などで取りこぼす)
        // ので、`modifiers.shift` を見た fallback も併用する。
        let shift_held_now = ctx.input(|i| i.modifiers.shift);
        let shift_enter = ctx.input_mut(|i| {
            let direct = i.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter);
            let fallback = shift_held_now && i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
            direct || fallback
        });
        if shift_enter {
            crate::logger::log("video Shift+Enter pressed → external player".to_string());
        }
        // Enter 単独: 再生 / 一時停止トグル。Shift+Enter は上で先に取っているので
        // ここでは shift 無しの Enter のみが残っている。
        let enter = !shift_held_now
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        // Phase 7.H シーク粒度: ←→=5 秒、Shift+←→=1 秒、Ctrl+←→=30 秒。
        // ↑↓ は consume せず後段の image arrow_up/down (= 前後ファイル) に流す
        // (= マウスホイールと整合)。Shift+↑↓ だけ動画モードで音量に使う。
        let ctrl_shift_left = ctx.input_mut(|i| {
            i.consume_key(
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                egui::Key::ArrowLeft,
            )
        });
        let ctrl_shift_right = ctx.input_mut(|i| {
            i.consume_key(
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                egui::Key::ArrowRight,
            )
        });
        let frame_step_key = ctrl_shift_left || ctrl_shift_right;
        let ctrl_shift_held_now = ctx.input(|i| i.modifiers.ctrl && i.modifiers.shift);
        let shift_left = !frame_step_key
            && !ctrl_shift_held_now
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowLeft));
        let shift_right = !frame_step_key
            && !ctrl_shift_held_now
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowRight));
        let ctrl_left = !frame_step_key
            && !ctrl_shift_held_now
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowLeft));
        let ctrl_right = !frame_step_key
            && !ctrl_shift_held_now
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowRight));
        let left = !frame_step_key
            && !ctrl_shift_held_now
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft));
        let right = !frame_step_key
            && !ctrl_shift_held_now
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight));
        let shift_up = ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowUp));
        let shift_down =
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowDown));
        // ↑↓ プレーンは consume しない (= image handler が file navigation に使う)。
        let mute_key = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::M));
        let loop_key = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::L));
        // Phase 5.4.1: B キーで現在位置にブックマーク追加 (動画モード限定)。
        // 画像モードの B (透過背景循環) とは handle_video_input 先行 consume で分離。
        let bookmark_key = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::B));
        // Phase 5.5: S キーでタイルモード トグル (動画モード限定)。画像モードの
        // S (スライドショー) とは handle_video_input 先行 consume で分離する。
        let tile_key = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::S));
        // Phase 8.I: P キーでフレームレート オーバーレイのトグル (動画モード限定)。
        // 画像モードの P (post-filter) とは handle_video_input 先行 consume で分離。
        let perf_key = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::P));
        // W キー: 頭出し (= seek to 0 + play)。左手で押しやすく、画像モードでも未使用。
        let rewind_key = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::W));
        // J/K: チャプター・ブックマーク・ピンを 1 本のマーカー列にまとめて前後ジャンプ。
        // 矢印キーは既に固定秒数シークに使っているので別キー。J=前、K=次。
        let prev_marker_key = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::J));
        let next_marker_key = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::K));
        // タイルモード中は ESC でも閉じれるようにする (= 一般的な「全画面モード解除」)。
        // ただし ESC は元々フルスクリーン全体を閉じるキーなので、タイルモード中だけ
        // 横取りする。
        let escape_for_tile = self.video_tile_state.is_some()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));

        if shift_enter {
            if let Some(p) = video_path {
                open_external_player(p);
            }
            return;
        }

        // 先に現在の音量だけ取り出す (player borrow を短く保つ)
        let cur_volume = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => player.volume(),
            _ => return,
        };
        // Phase 7.H: 音量は Shift+↑↓ 限定 (= 20% step)。プレーン ↑↓ はファイル移動。
        let new_vol = if shift_up {
            Some((cur_volume + 0.20).min(crate::settings::VIDEO_VOLUME_MAX))
        } else if shift_down {
            Some((cur_volume - 0.20).max(0.0))
        } else {
            None
        };

        // player に作用させる (借用はこの if-let のスコープ内で完結)
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            if enter {
                player.toggle_play();
            }
            if ctrl_shift_left {
                player.step_frame(-1);
            }
            if ctrl_shift_right {
                player.step_frame(1);
            }
            // Phase 7.H シーク粒度:
            //   ←→ = 5 秒 (デフォルト、動画プレイヤー慣例)
            //   Shift+←→ = 1 秒 (細かい、フレーム単位調整に近い)
            //   Ctrl+←→ = 30 秒 (大きい、長い動画の早送り用)
            if left {
                player.seek_relative(-5.0);
            }
            if right {
                player.seek_relative(5.0);
            }
            if shift_left {
                player.seek_relative(-1.0);
            }
            if shift_right {
                player.seek_relative(1.0);
            }
            if ctrl_left {
                player.seek_relative(-30.0);
            }
            if ctrl_right {
                player.seek_relative(30.0);
            }
            if let Some(v) = new_vol {
                player.set_volume(v);
            }
            if mute_key {
                player.set_muted(!player.is_muted());
            }
        }

        // 設定への反映 (player 借用は終わっているので self.settings を書き換え可能)。
        // Phase 8 (Codex P3-1): 即時 save 化。グリッド列数や動画タイル列数が
        // 即時保存されるのと挙動を揃え、強制終了時の loss を防ぐ。
        if let Some(v) = new_vol {
            self.settings.video_volume = v;
            self.settings.save();
        }
        if loop_key {
            self.settings.video_loop = !self.settings.video_loop;
            self.settings.save();
        }

        // Phase 5.5: S キーでタイルモード トグル。画面サイズは Context 側から取得。
        // toggle 内で fs_cache / video_tile_state を借用するので、player 借用後に呼ぶ。
        if tile_key {
            let screen = ctx.content_rect().size();
            self.toggle_video_tile_mode(fs_idx, screen);
        }
        if perf_key {
            self.video_perf_overlay_visible = !self.video_perf_overlay_visible;
            // 切替時に履歴を一旦クリア (= ON 直後は素のグラフから始まる)。
            self.reset_video_perf_history();
        }
        if rewind_key && let Some(p) = self.fs_video_player(fs_idx) {
            p.seek(0.0);
            if !p.is_playing() {
                p.toggle_play();
            }
        }

        // 何らかの動画ショートカット入力があれば HUD のフェードタイマをリセット
        // (= HUD を再表示)。マウス活動と同様の扱い。
        let any_video_key = enter
            || left
            || right
            || shift_left
            || shift_right
            || ctrl_left
            || ctrl_right
            || ctrl_shift_left
            || ctrl_shift_right
            || shift_up
            || shift_down
            || mute_key
            || loop_key
            || bookmark_key
            || tile_key
            || perf_key
            || rewind_key
            || prev_marker_key
            || next_marker_key;
        if any_video_key {
            self.video_hud_last_activity = Some(std::time::Instant::now());
        }

        // J/K: マーカー (チャプター/ブックマーク/ピン) 間の前後ジャンプ。
        // 現在再生位置 ± epsilon を境にした最近接探索で「現在マーカーで足踏み」を防ぐ。
        // ジャンプ先がなければ何もしない (= 端で止まる)。
        if prev_marker_key || next_marker_key {
            let markers = self.collect_video_nav_markers(fs_idx);
            let current = self
                .fs_video_player(fs_idx)
                .map(|p| p.position())
                .unwrap_or(0.0);
            let target: Option<NavMarker> = if next_marker_key {
                markers
                    .iter()
                    .find(|m| m.pts > current + NAV_MARKER_EPSILON)
                    .cloned()
            } else {
                markers
                    .iter()
                    .rev()
                    .find(|m| m.pts < current - NAV_MARKER_EPSILON)
                    .cloned()
            };
            if let Some(m) = target {
                if let Some(p) = self.fs_video_player(fs_idx) {
                    p.seek(m.pts);
                }
                let direction = if next_marker_key { "次の" } else { "前の" };
                let kind_label = match m.kind {
                    NavMarkerKind::Chapter => "チャプター",
                    NavMarkerKind::Bookmark => "ブックマーク",
                    NavMarkerKind::Pin => "ピン",
                };
                let toast = match (m.kind, m.title.as_deref()) {
                    (NavMarkerKind::Chapter, Some(t)) | (NavMarkerKind::Bookmark, Some(t))
                        if !t.is_empty() =>
                    {
                        format!(
                            "{} {}{}: {}",
                            crate::ui_helpers::format_hms(m.pts),
                            direction,
                            kind_label,
                            t
                        )
                    }
                    _ => format!(
                        "{} {}{}",
                        crate::ui_helpers::format_hms(m.pts),
                        direction,
                        kind_label
                    ),
                };
                self.show_feedback_toast(toast);
            }
        }
        // ESC は タイルモード中のみキャッチして close。フルスクリーン解脱は呼び出し側
        // (handle_image_keys 後段) の通常 ESC で扱う。
        if escape_for_tile {
            self.video_tile_state = None;
            self.video_tile_swap_pending = None;
        }

        // Phase 5.4.1: ブックマーク追加。現在位置 + 動画パスを取得して DB に挿入。
        // 借用衝突を避けるため `if let Some(player)` 短いスコープで pos / path を抜く。
        // Codex P5.4 M2 反映: Loading / エラー状態では追加させない (= 0.0s に
        // ゴミブックマークが入るのを防ぐ)。info() が来ている = duration / has_audio が
        // 確定 = 1 フレーム以上は decoder が動いている前提。
        if bookmark_key {
            let snapshot = match self.fs_cache.get(&fs_idx) {
                Some(FsCacheEntry::Video { player, .. }) => {
                    if player.error().is_some() || player.info().is_none() {
                        None
                    } else {
                        Some((player.path().clone(), player.position()))
                    }
                }
                _ => None,
            };
            if let (Some((path, pts)), Some(db)) = (snapshot, self.video_bookmark_db.as_ref()) {
                if let Err(e) = db.add(&path, pts, None, &[]) {
                    crate::logger::log(format!("video bookmark add failed: {e}"));
                } else {
                    crate::logger::log(format!(
                        "video bookmark added: pts={pts:.2}s {}",
                        path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                    ));
                    self.refresh_fullscreen_video_marker_cache(fs_idx);
                    #[cfg(windows)]
                    self.sync_native_video_timeline_markers(fs_idx);
                }
            }
        }
    }

    /// Perf graph の history と関連トラッキング状態を全部クリア。
    /// 速度変更 / overlay トグル時に呼ぶ。旧速度サンプルが新スケールで誤色表示
    /// される現象を回避する。
    pub(crate) fn reset_video_perf_history(&mut self) {
        self.video_perf_history.clear();
        self.video_perf_last_wall = None;
        self.video_perf_last_seq = None;
        self.video_perf_last_decoder_skip = None;
        self.video_perf_last_ui_skip = None;
        self.video_perf_pause_start = None;
    }

    /// 動画 VideoPlayer の `displayed_frame_seq` (= GPU/CPU 経路問わず tick で +1)
    /// の変化で frame interval (ms) を履歴に記録。skip delta は decoder 側
    /// `dropped_full` と UI 側 `dropped_past` に分けて記録し、色分け表示する。
    /// 期待 interval (= 1000/fps) の 3x を超える値は「再生開始 / seek / pause 復帰
    /// 等の transient による wall 待ち時間」とみなして履歴に入れない。
    pub(crate) fn sample_video_perf(&mut self, fs_idx: usize) {
        let snapshot: Option<(u64, u64, u64, usize, f32, bool, bool)> =
            match self.fs_cache.get(&fs_idx) {
                Some(crate::fs_animation::FsCacheEntry::Video { player, .. }) => {
                    let (decoder_skips, ui_skips) = player.skip_counters();
                    // 期待 interval は再生速度に追従する: 30fps を 0.5x 再生なら
                    // 実フレーム間隔は 66.7ms、2x なら 16.7ms。
                    let avg_fps = player
                        .info()
                        .map(|i| i.avg_fps as f32)
                        .filter(|fps| *fps > 0.5 && fps.is_finite())
                        .unwrap_or(30.0);
                    let playback_speed = (player.playback_speed() as f32).max(0.1);
                    let expected_ms = 1000.0 / (avg_fps * playback_speed);
                    let state = player.engine_state_code();
                    let is_warmup = state != crate::video::engine::actor::state_code::PLAYING;
                    // Phase 9.G: graph freeze 判定 = pause OR seeking。
                    // - PAUSED: ユーザー一時停止中
                    // - is_seeking() (= override active、demux が seek 処理中で
                    //   post-seek 1 枚目が UI に届くまで): 「黒い空間」を防ぐ
                    let is_paused_or_seeking = player.is_paused_or_seeking();
                    Some((
                        player.displayed_frame_seq(),
                        decoder_skips,
                        ui_skips,
                        player.pending_frames(),
                        expected_ms,
                        is_warmup,
                        is_paused_or_seeking,
                    ))
                }
                _ => None,
            };
        let Some((
            cur_seq,
            cur_decoder_skip,
            cur_ui_skip,
            cur_buf,
            expected_ms,
            is_warmup,
            is_freeze,
        )) = snapshot
        else {
            return;
        };
        // Phase 9.F/G: freeze→resume 検出 + history の arrival shift。
        //
        // freeze の対象:
        //   - 一時停止 (engine state == PAUSED): ユーザー操作
        //   - シーク処理中 (clock.is_seeking() = override 設定中): demux が seek
        //     を処理して post-seek 1 枚目が UI に届くまでの「黒い空間」期間
        //
        // freeze 中は perf graph の `now` を最新 arrival で固定しているが、
        // resume 時に Instant::now() に切り替わると graph が一気に進んで「ジャンプ」
        // する。freeze 期間分だけ history の arrival を未来へ進めて、視覚位置を
        // そのままに保つ。さらに freeze 中は新規サンプル収集も停止 (= seek 処理中の
        // 「黒い空間」を生まない、ユーザー要望)。
        let now_real = std::time::Instant::now();
        if is_freeze {
            // playing → freeze (or freeze 継続): 開始時刻を記録 (or 維持) して
            // サンプル収集を停止。
            if self.video_perf_pause_start.is_none() {
                self.video_perf_pause_start = Some(now_real);
            }
            return;
        }
        if let Some(pause_start) = self.video_perf_pause_start.take() {
            // freeze → playing: 期間分 offset を計算して history を shift。
            let pause_dur = now_real.saturating_duration_since(pause_start);
            if pause_dur > std::time::Duration::from_millis(10) {
                for entry in self.video_perf_history.iter_mut() {
                    entry.arrival += pause_dur;
                }
                if let Some(prev) = self.video_perf_last_wall.as_mut() {
                    *prev += pause_dur;
                }
            }
        }
        if Some(cur_seq) != self.video_perf_last_seq {
            let now = std::time::Instant::now();
            if let (Some(prev_wall), Some(_), Some(prev_decoder_skip), Some(prev_ui_skip)) = (
                self.video_perf_last_wall,
                self.video_perf_last_seq,
                self.video_perf_last_decoder_skip,
                self.video_perf_last_ui_skip,
            ) {
                let interval_ms = now.saturating_duration_since(prev_wall).as_secs_f32() * 1000.0;
                let decoder_delta = cur_decoder_skip.saturating_sub(prev_decoder_skip) as u32;
                let ui_delta = cur_ui_skip.saturating_sub(prev_ui_skip) as u32;
                let buf_clamped = cur_buf.min(255) as u8;
                // Keep short slow-playback samples in the graph so "expected
                // frame opportunities missed" is visible even when decoder/UI
                // drop counters stay at zero. Longer stalls are still treated
                // as transient pauses and skipped.
                let transient_threshold = (expected_ms * 8.0).max(250.0);
                if interval_ms <= transient_threshold {
                    let hitch_ms = (expected_ms * 1.5).max(20.0);
                    let expected_misses = if !is_warmup && interval_ms > hitch_ms {
                        (interval_ms / expected_ms).round() as u32
                    } else {
                        1
                    }
                    .saturating_sub(1);
                    self.video_perf_history
                        .push_back(crate::app::VideoPerfSample {
                            interval_ms,
                            arrival: now,
                            expected_misses,
                            decoder_skips: decoder_delta,
                            ui_skips: ui_delta,
                            buffer_len: buf_clamped,
                            is_warmup,
                        });
                    if crate::perf::is_enabled()
                        && (expected_misses > 0 || decoder_delta > 0 || ui_delta > 0)
                    {
                        crate::perf::event(
                            "video",
                            "display_miss",
                            None,
                            0,
                            &[
                                ("fs_idx", serde_json::Value::from(fs_idx as i64)),
                                ("seq", serde_json::Value::from(cur_seq as i64)),
                                ("interval_ms", serde_json::Value::from(interval_ms)),
                                ("expected_ms", serde_json::Value::from(expected_ms)),
                                ("hitch_ms", serde_json::Value::from(hitch_ms)),
                                ("expected_misses", serde_json::Value::from(expected_misses)),
                                ("decoder_skips", serde_json::Value::from(decoder_delta)),
                                ("ui_skips", serde_json::Value::from(ui_delta)),
                                ("buffer_len", serde_json::Value::from(buf_clamped)),
                                ("is_warmup", serde_json::Value::from(is_warmup)),
                            ],
                        );
                    }
                    // Phase 8.K: 容量 200 だと 60fps で 3.3 秒分しか保持できず、
                    // graph の WINDOW_SECS=6.0 に対し左 半分以上が空欄になる。
                    // 6 秒 × 100fps の余裕を持たせて 600 に拡大。
                    while self.video_perf_history.len() > 600 {
                        self.video_perf_history.pop_front();
                    }
                }
            }
            self.video_perf_last_wall = Some(now);
            self.video_perf_last_seq = Some(cur_seq);
            self.video_perf_last_decoder_skip = Some(cur_decoder_skip);
            self.video_perf_last_ui_skip = Some(cur_ui_skip);
        }
    }

    /// FPS / フレーム間隔のオーバーレイ。直近 200 frame の interval (ms) を折れ線で、
    /// 動画 fps から期待値の 1.5x 超を赤縦線 (hitch) で目立たせる。左上半透明。
    pub(crate) fn draw_video_perf_overlay(&self, ui: &mut egui::Ui, full_rect: egui::Rect) {
        let painter = ui.painter().clone();
        let (video_info, playback_speed) = self
            .fullscreen_idx
            .and_then(|idx| match self.fs_cache.get(&idx) {
                Some(crate::fs_animation::FsCacheEntry::Video { player, .. }) => {
                    Some((player.info().cloned(), player.playback_speed() as f32))
                }
                _ => None,
            })
            .unwrap_or((None, 1.0));
        // 動画の fps と再生速度から実 frame interval (ms) を計算する。
        // 0.5x なら 30fps→66.7ms、2x なら→16.7ms。Y 軸は実 interval を基準に
        // スケーリングする (= 速度変更で hitch 閾値が追従する)。
        let avg_fps: f32 = video_info
            .as_ref()
            .map(|i| i.avg_fps as f32)
            .filter(|fps| *fps > 0.5 && fps.is_finite())
            .unwrap_or(30.0);
        let speed = playback_speed.max(0.1);
        let expected_ms: f32 = 1000.0 / (avg_fps * speed);
        // hitch 閾値は期待値の 1.5x (= 50% 超過 = 1 frame 落ち相当)。
        let hitch_ms: f32 = (expected_ms * 1.5).max(20.0);
        // 縦軸上限は期待値の 2x、ただし最小 50ms (= 60fps 基準でも見やすい)。
        let y_max_ms: f32 = (expected_ms * 2.0).max(50.0);

        const W: f32 = 430.0;
        const H: f32 = 124.0; // codec 行 + 主グラフ + 下部 buffer strip
        let rect = egui::Rect::from_min_size(
            egui::pos2(full_rect.min.x + 8.0, full_rect.min.y + 8.0),
            egui::vec2(W, H),
        );
        painter.rect_filled(
            rect,
            4.0,
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 170),
        );
        painter.rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(1.0, egui::Color32::from_gray(140)),
            egui::StrokeKind::Inside,
        );

        // ヘッダ統計
        let n = self.video_perf_history.len();
        if n == 0 {
            let msg = format!(
                "Perf: collecting…  last_seq={:?}  wall={}",
                self.video_perf_last_seq,
                if self.video_perf_last_wall.is_some() {
                    "set"
                } else {
                    "none"
                }
            );
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                msg,
                egui::FontId::proportional(11.0),
                egui::Color32::WHITE,
            );
            return;
        }
        let avg: f32 = self
            .video_perf_history
            .iter()
            .map(|s| s.interval_ms)
            .sum::<f32>()
            / n as f32;
        let max: f32 = self
            .video_perf_history
            .iter()
            .map(|s| s.interval_ms)
            .fold(0.0_f32, f32::max);
        let decoder_skips: u32 = self
            .video_perf_history
            .iter()
            .map(|s| s.decoder_skips)
            .sum();
        let ui_skips: u32 = self.video_perf_history.iter().map(|s| s.ui_skips).sum();
        let expected_misses: u32 = self
            .video_perf_history
            .iter()
            .map(|s| s.expected_misses)
            .sum();
        let cur_buf = self
            .video_perf_history
            .back()
            .map(|s| s.buffer_len)
            .unwrap_or(0);
        // ラベルは "frame 到着間隔のばらつき (= jitter)" を示すと意味付け。
        // 平均値は wall rate と一致するのが正常で、max が target を超えるほど
        // pipeline の変動が大きい。
        // Phase 8.K: header 文字列を短縮 (360px 枠を超えないため)。
        // "60.0fps/16.7ms  jit 17.0/25.5  miss:2 d:1 ui:5  buf:3/24"
        let header = format!(
            "{:.1}fps/{:.1}ms  jit {:.1}/{:.1}  miss:{} d:{} ui:{}  buf:{}/{}",
            1000.0 / expected_ms,
            expected_ms,
            avg,
            max,
            expected_misses,
            decoder_skips,
            ui_skips,
            cur_buf,
            crate::video::MAX_RENDER_QUEUE,
        );
        painter.text(
            egui::pos2(rect.min.x + 6.0, rect.min.y + 4.0),
            egui::Align2::LEFT_TOP,
            header,
            egui::FontId::proportional(11.0),
            egui::Color32::WHITE,
        );
        if let Some(info) = video_info.as_ref() {
            let decode = if info.hw_decode_active { "HW" } else { "SW" };
            let path = if info.gpu_path_active { "GPU" } else { "CPU" };
            let d3d11 = if info.d3d11va_supported { "yes" } else { "no" };
            let codec_line = format!(
                "codec {} / {}  {}/{}  D3D11VA:{}",
                info.video_codec, info.video_decoder, decode, path, d3d11
            );
            painter.text(
                egui::pos2(rect.min.x + 6.0, rect.min.y + 18.0),
                egui::Align2::LEFT_TOP,
                codec_line,
                egui::FontId::proportional(10.0),
                egui::Color32::from_rgb(210, 230, 255),
            );
        }

        // グラフ領域: 上部 (interval) + 下部 (buffer strip) に分割。
        // 中央に細い区切り線を入れて視覚的に分離。
        let main_top = rect.min.y + 36.0;
        let strip_h = 14.0;
        let strip_top = rect.max.y - 4.0 - strip_h;
        let graph = egui::Rect::from_min_max(
            egui::pos2(rect.min.x + 6.0, main_top),
            egui::pos2(rect.max.x - 6.0, strip_top - 2.0),
        );
        let strip = egui::Rect::from_min_max(
            egui::pos2(rect.min.x + 6.0, strip_top),
            egui::pos2(rect.max.x - 6.0, strip_top + strip_h),
        );
        let y_for = |ms: f32| -> f32 {
            graph.max.y - (ms.clamp(0.0, y_max_ms) / y_max_ms) * graph.height()
        };
        // ガイドライン: 期待値、期待値 (= target、黄)、hitch 閾値 (赤)。
        // 追加で半分の値 (= 60Hz 表示時の vsync 周期 16.7ms 等) も控えめに描く。
        for &(ms, color) in &[
            (expected_ms * 0.5, egui::Color32::from_rgb(80, 200, 120)),
            (expected_ms, egui::Color32::from_rgb(200, 200, 80)),
            (hitch_ms, egui::Color32::from_rgb(220, 100, 100)),
        ] {
            let y = y_for(ms);
            painter.line_segment(
                [egui::pos2(graph.min.x, y), egui::pos2(graph.max.x, y)],
                egui::Stroke::new(
                    0.5,
                    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 140),
                ),
            );
            painter.text(
                egui::pos2(graph.max.x - 2.0, y - 1.0),
                egui::Align2::RIGHT_BOTTOM,
                format!("{:.1}", ms),
                egui::FontId::proportional(9.0),
                egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 200),
            );
        }
        // X 軸: **wall 経過時間ベースで連続スクロール**。
        // - WINDOW_SECS 秒分のグラフを表示 (= graph_w を WINDOW_SECS で割った px/s)
        // - 各サンプルの x = right_edge - (now - sample.arrival) * px_per_sec
        // - repaint ごとに px_per_sec * dt だけ左へスクロール (= サブピクセル粒度)
        // 旧実装はサンプル数 index で X を決めていたため、新サンプルが届く瞬間
        // (= 16-33ms 間隔) に discrete に進むだけで stair-step がカクついていた。
        const WINDOW_SECS: f32 = 6.0;
        let px_per_sec = graph.width() / WINDOW_SECS;
        // Phase 9.E/G: 一時停止 / シーク処理中はグラフのスクロールを止める。
        // 旧挙動: `now = Instant::now()` を毎 frame 計算するため、pause / seek 中も
        // 時間が進み、サンプルは左に流れて画面外に消える + seek 後に「黒い空間」が
        // できる。
        // 新挙動: pause/seek 中は freeze 開始時刻 (`video_perf_pause_start`、
        // sample_video_perf が立てる) を `now` として使い、graph を凍結。
        // post-seek 1 枚目が来た時点で再開すると同時に history の arrival を
        // freeze 期間分 shift してジャンプを抑止 (sample_video_perf 側)。
        //
        // 旧版は `last_sample.arrival` に snap していたが、is_paused_or_seeking が
        // 一瞬だけ true → false に揺れる場面 (= 速度変更や transient state) で、
        // 1 frame だけ「`now` が直近 sample の arrival に snap」する → 次 frame で
        // real `Instant::now()` に戻る、を繰り返してグラフがちらつく問題があった。
        // freeze 開始時の real time を保持することで、freeze 入退出時の `now` の
        // 不連続をなくす (= 直前の real `now` と連続的に繋がる)。
        let is_freeze = self
            .fullscreen_idx
            .and_then(|idx| match self.fs_cache.get(&idx) {
                Some(crate::fs_animation::FsCacheEntry::Video { player, .. }) => {
                    Some(player.is_paused_or_seeking())
                }
                _ => None,
            })
            .unwrap_or(false);
        let now = if is_freeze {
            self.video_perf_pause_start
                .unwrap_or_else(std::time::Instant::now)
        } else {
            std::time::Instant::now()
        };
        let x_for = |arrival: std::time::Instant| -> f32 {
            let dt = now.saturating_duration_since(arrival).as_secs_f32();
            graph.max.x - dt * px_per_sec
        };

        // 連続描画のために repaint を ~16ms (= 60Hz) で予約。
        // ただし freeze 中 (= pause / seek) は再描画しても画面は変わらないので
        // 1Hz に落として CPU 節約。
        let repaint_interval = if is_freeze {
            std::time::Duration::from_millis(1000)
        } else {
            std::time::Duration::from_millis(16)
        };
        ui.ctx().request_repaint_after(repaint_interval);

        // graph 矩形外にはみ出さないよう painter を clip。
        let painter = painter.with_clip_rect(graph);

        // ── Warmup region 背景タイント (Phase 9.B、2026-04-30 追加) ──
        //
        // engine state ≠ Playing (= Loading / Buffering / Seeking / Paused / Eof)
        // の連続 sample 範囲を緑がかった半透明矩形で塗る。動画 open / W キー戻り /
        // シーク直後に「クロックは凍結 + 音声 silence + 表示 frame は固定」状態が
        // 数十-数百 ms 続く期間 (= "warmup") をユーザーが視認できるようにする。
        //
        // この warmup 区間中は意図的に頻繁な dropped_past が起きない設計
        // (= cpal 出力が silent なので audio anchor が pace を進めない、UI 表示も
        // 凍結 frame のみ)。区間終了 (= Playing 遷移) と同時に滑らかな再生が始まる。
        {
            let warmup_color = egui::Color32::from_rgba_unmultiplied(80, 200, 120, 32);
            let draw_run = |start: std::time::Instant, end: std::time::Instant| {
                let x_start = x_for(start);
                let x_end = x_for(end);
                let lo = x_start.min(x_end);
                let hi = x_start.max(x_end).max(lo + 2.0);
                let r = egui::Rect::from_min_max(
                    egui::pos2(lo, graph.min.y),
                    egui::pos2(hi, graph.max.y),
                );
                painter.rect_filled(r, 0.0, warmup_color);
            };
            // 微小なギャップを 1 つの run として merge するため、隣接 sample の
            // arrival_dt を見て threshold (~50ms) を超えたら run を切る。
            const RUN_GAP_THRESHOLD_MS: f32 = 50.0;
            let mut run_start: Option<std::time::Instant> = None;
            let mut last_arrival: Option<std::time::Instant> = None;
            for sample in &self.video_perf_history {
                if sample.is_warmup {
                    if run_start.is_none() {
                        run_start = Some(sample.arrival);
                    } else if let Some(last) = last_arrival {
                        let gap_ms =
                            sample.arrival.saturating_duration_since(last).as_secs_f32() * 1000.0;
                        if gap_ms > RUN_GAP_THRESHOLD_MS {
                            // 過去 run を flush して新 run を開始
                            if let Some(start) = run_start {
                                draw_run(start, last);
                            }
                            run_start = Some(sample.arrival);
                        }
                    }
                    last_arrival = Some(sample.arrival);
                } else if let Some(start) = run_start {
                    // run 終了: 矩形を描画してリセット
                    let end = last_arrival.unwrap_or(sample.arrival);
                    draw_run(start, end);
                    run_start = None;
                    last_arrival = None;
                }
            }
            // 履歴末尾が warmup 中の場合、現在 run を最右端まで描画。
            if let (Some(start), Some(end)) = (run_start, last_arrival) {
                draw_run(start, end);
            }
        }

        // skip 縦線:
        // - expected miss: source FPS から見て到着しなかった表示機会。太い赤で表示。
        // - decoder dropped_full: video_tx overflow。濃い赤で表示。
        // - UI dropped_past: tick が複数 displayable frame をまとめて消費。橙で表示。
        // 同一サンプルで両方発生した場合も見えるよう、横に少しずらして描く。
        for sample in &self.video_perf_history {
            if sample.expected_misses > 0 {
                let alpha = (145 + (sample.expected_misses * 35).min(110)) as u8;
                let x = x_for(sample.arrival);
                painter.line_segment(
                    [egui::pos2(x, graph.min.y), egui::pos2(x, graph.max.y)],
                    egui::Stroke::new(
                        2.0,
                        egui::Color32::from_rgba_unmultiplied(255, 45, 45, alpha),
                    ),
                );
            }
            if sample.decoder_skips > 0 {
                let alpha = (130 + (sample.decoder_skips * 45).min(125)) as u8;
                let x = x_for(sample.arrival) - 0.8;
                painter.line_segment(
                    [egui::pos2(x, graph.min.y), egui::pos2(x, graph.max.y)],
                    egui::Stroke::new(
                        1.5,
                        egui::Color32::from_rgba_unmultiplied(255, 70, 90, alpha),
                    ),
                );
            }
            if sample.ui_skips > 0 {
                let alpha = (120 + (sample.ui_skips * 35).min(135)) as u8;
                let x = x_for(sample.arrival) + 0.8;
                painter.line_segment(
                    [egui::pos2(x, graph.min.y), egui::pos2(x, graph.max.y)],
                    egui::Stroke::new(
                        1.3,
                        egui::Color32::from_rgba_unmultiplied(255, 170, 70, alpha),
                    ),
                );
            }
        }
        // 折れ線: 各 frame の到着間隔 (= UI が体感する処理時間)。
        // 右端が最新、左に向かって過去。hitch 閾値を超えるセグメントは色を変えて
        // 「期待値より遅い」状態が一目で分かるようにする。
        if n > 1 {
            let mut prev: Option<(egui::Pos2, f32)> = None;
            for sample in &self.video_perf_history {
                let x = x_for(sample.arrival);
                let p = egui::pos2(x, y_for(sample.interval_ms));
                if let Some((prev_p, prev_v)) = prev {
                    if !(p.x < graph.min.x && prev_p.x < graph.min.x) {
                        let exceeds = sample.interval_ms > hitch_ms || prev_v > hitch_ms;
                        let color = if exceeds {
                            egui::Color32::from_rgb(255, 200, 100)
                        } else {
                            egui::Color32::from_rgb(180, 230, 255)
                        };
                        painter.line_segment([prev_p, p], egui::Stroke::new(1.2, color));
                    }
                }
                prev = Some((p, sample.interval_ms));
            }
        }
        // 上下グラフの境界線。
        let painter_root = ui.painter().clone();
        painter_root.line_segment(
            [
                egui::pos2(strip.min.x, strip.min.y - 1.0),
                egui::pos2(strip.max.x, strip.min.y - 1.0),
            ],
            egui::Stroke::new(0.5, egui::Color32::from_gray(80)),
        );

        // ── 下部 buffer strip: future_frames キュー残量 (= 表示待ち) ──
        // 高さ 14px の縦棒で 0..MAX_RENDER_QUEUE をスケール。0 = starvation 危険、
        // 満杯 = decoder 過剰生産。skip 赤縦線の真下を見ると context が分かる:
        //   - 赤線 + buf=0  → UI starvation (= queue 空で frame 不足)
        //   - 赤線 + buf 満杯 → decoder 側 dropped_full (= channel overflow)
        let strip_painter = painter_root.with_clip_rect(strip);
        // 背景うっすら
        strip_painter.rect_filled(
            strip,
            0.0,
            egui::Color32::from_rgba_unmultiplied(20, 20, 30, 200),
        );
        let max_buf = crate::video::MAX_RENDER_QUEUE.max(1) as f32;
        let bar_w = (strip.width() / 200.0).max(1.0); // 1 sample あたりの幅
        for sample in &self.video_perf_history {
            let x = graph.max.x - {
                let dt = now.saturating_duration_since(sample.arrival).as_secs_f32();
                dt * px_per_sec
            };
            if x < strip.min.x - bar_w || x > strip.max.x {
                continue;
            }
            let level = (sample.buffer_len as f32 / max_buf).clamp(0.0, 1.0);
            // Phase 8.K: buf=0 では level=0 で bar 高さ 0 になり描画されない。
            // starvation を視認できるよう strip 全高に minimum-height (= 全高) で
            // 描く。0 < buf でも視認性のため最低 2px は確保。
            let bar_h = if sample.buffer_len == 0 {
                strip.height()
            } else {
                (level * strip.height()).max(2.0)
            };
            let bar = egui::Rect::from_min_max(
                egui::pos2(x - bar_w * 0.5, strip.max.y - bar_h),
                egui::pos2(x + bar_w * 0.5, strip.max.y),
            );
            // buf 残量で色を変化: 緑 (健全) → 黄 (中) → 赤 (危険、starvation 寸前)
            let color = if sample.buffer_len == 0 {
                egui::Color32::from_rgb(220, 80, 80)
            } else if level < 0.25 {
                egui::Color32::from_rgb(220, 200, 80)
            } else {
                egui::Color32::from_rgb(120, 200, 130)
            };
            strip_painter.rect_filled(bar, 0.0, color);
        }
        // strip ラベル (= "buf")
        painter_root.text(
            egui::pos2(strip.min.x - 2.0, strip.center().y),
            egui::Align2::RIGHT_CENTER,
            "",
            egui::FontId::proportional(9.0),
            egui::Color32::from_gray(160),
        );
    }

    /// 動画再生時のホバー HUD を描画する。下部 44px に play/pause / seek bar /
    /// mute / volume slider を配置し、それぞれ独立した `ui.interact` でクリック・
    /// ドラッグを処理する。HUD 領域に当たるクリックは `video_hud_rect` 経由で
    /// `handle_fs_wheel_and_click` 側の toggle_play から除外する。
    pub(crate) fn draw_video_hud(
        &mut self,
        ui: &mut egui::Ui,
        _ctx: &egui::Context,
        full_rect: egui::Rect,
        fs_idx: usize,
    ) {
        let (is_playing, position, duration, volume, muted, playback_speed, has_texture, has_error) =
            match self.fs_cache.get(&fs_idx) {
                Some(FsCacheEntry::Video { player, .. }) => (
                    player.is_playing(),
                    player.position(),
                    player.duration(),
                    player.volume(),
                    player.is_muted(),
                    player.playback_speed(),
                    // 1 枚以上のフレームが decoder から供給され UI tick が認識した
                    // 時点で「描画コンテンツが揃った」とみなす (= "動画を準備中..."
                    // を抜けて通常表示に切り替える)。native presenter / 旧 egui 経路の
                    // 違いに依存しない atomic counter ベースの判定。
                    player.displayed_frame_seq() > 0,
                    player.error().map(|s| s.to_string()),
                ),
                _ => return,
            };

        // ── エラー表示 ──
        if let Some(err) = has_error {
            let painter = ui.painter();
            let galley = painter.layout_no_wrap(
                format!("動画を再生できません: {err}"),
                egui::FontId::proportional(20.0),
                egui::Color32::from_rgb(255, 120, 120),
            );
            let pos = full_rect.center() - galley.size() / 2.0;
            let bg_rect = egui::Rect::from_min_size(pos, galley.size()).expand(12.0);
            painter.rect_filled(
                bg_rect,
                6.0,
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 200),
            );
            painter.galley(pos, galley, egui::Color32::from_rgb(255, 120, 120));
            return;
        }

        // ── まだ最初のフレームが届いていない: 中央にスピナー ──
        if !has_texture {
            let painter = ui.painter();
            let galley = painter.layout_no_wrap(
                "動画を準備中...".to_string(),
                egui::FontId::proportional(18.0),
                egui::Color32::WHITE,
            );
            let pos = full_rect.center() - galley.size() / 2.0;
            painter.galley(pos, galley, egui::Color32::WHITE);
            return;
        }

        // 一時停止中の中央 2 ボタン (「最初から」「続きから」) は native presenter
        // 専用 (overlay_draw.rs::draw_native_center_pause_controls)。legacy egui
        // presenter は 6d3ba5e で drop 済みなので、ここでは描画しない。

        // 再生中、ユーザーが一定時間操作しなかったら HUD を滑らかに薄くしていく。
        // 一時停止中も last_activity を最新化することで、再生再開直後に 2 秒は表示が
        // 保証される (動画切替直後の初回フレームも同様)。
        let hud_rect = video_hud_rect(full_rect);
        let now = std::time::Instant::now();
        let pointer_active = ui.ctx().input(|i| {
            let in_hud = i.pointer.hover_pos().is_some_and(|p| hud_rect.contains(p));
            in_hud
                || i.pointer.is_decidedly_dragging()
                || i.pointer.any_click()
                || i.pointer.any_pressed()
                || i.pointer.velocity().length() > 0.5
        });
        if pointer_active || !is_playing {
            self.video_hud_last_activity = Some(now);
        }
        let alpha = if !is_playing {
            1.0
        } else {
            let last = *self.video_hud_last_activity.get_or_insert(now);
            let idle = now.duration_since(last).as_secs_f32();
            if idle < VIDEO_HUD_IDLE_BEFORE_FADE {
                1.0
            } else if idle < VIDEO_HUD_IDLE_BEFORE_FADE + VIDEO_HUD_FADE_DURATION {
                1.0 - (idle - VIDEO_HUD_IDLE_BEFORE_FADE) / VIDEO_HUD_FADE_DURATION
            } else {
                0.0
            }
        };
        self.video_hud_visible_factor = alpha;
        if alpha < 0.01 {
            // 完全フェード後はクリックを映像本体に届かせるため描画自体スキップ。
            return;
        }
        if alpha < 1.0 {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(33));
            ui.set_opacity(alpha);
        }

        // ── 下部 HUD バー ──
        let painter = ui.painter().clone();
        painter.rect_filled(
            hud_rect,
            0.0,
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 160),
        );

        // レイアウト計算 (左から): [play/pause][time][seek bar][mute][speed][vol slider][vol %]
        let pad = 8.0;
        let btn_size = 28.0;
        let cy = hud_rect.center().y;
        let time_font = egui::FontId::proportional(14.0);
        let label_font = egui::FontId::proportional(13.0);

        // ── ⏮ 最初から再生ボタン (Phase 6) ──
        let replay_rect = egui::Rect::from_min_size(
            egui::pos2(hud_rect.min.x + pad, cy - btn_size / 2.0),
            egui::vec2(btn_size, btn_size),
        );
        let replay_resp = ui.interact(
            replay_rect,
            egui::Id::new(("video_replay", fs_idx)),
            egui::Sense::click(),
        );
        draw_hud_button_bg(&painter, replay_rect, replay_resp.hovered());
        // ⏮ アイコン (縦バー + 左向き三角 = "skip back to start")。
        // Phase 7: 視認性のため再生 ▶ アイコンと同じ 0.4*btn_size サイズに。
        draw_replay_icon(&painter, replay_rect.center(), btn_size * 0.4);
        let replay_resp = replay_resp.on_hover_text("最初から再生 (頭出し + 即再生) [W]");
        if replay_resp.clicked()
            && let Some(p) = self.fs_video_player(fs_idx)
        {
            p.seek(0.0);
            // 「最初から再生」ボタンなので即時に再生開始する。
            if !p.is_playing() {
                p.toggle_play();
            }
        }

        // ── 再生 / 一時停止 ボタン ──
        let play_rect = egui::Rect::from_min_size(
            egui::pos2(replay_rect.max.x + pad, cy - btn_size / 2.0),
            egui::vec2(btn_size, btn_size),
        );
        let play_resp = ui.interact(
            play_rect,
            egui::Id::new(("video_play", fs_idx)),
            egui::Sense::click(),
        );
        draw_hud_button_bg(&painter, play_rect, play_resp.hovered());
        if is_playing {
            draw_pause_icon(&painter, play_rect.center(), btn_size * 0.32);
        } else {
            draw_play_triangle(&painter, play_rect.center(), btn_size * 0.4);
        }
        let play_resp = play_resp.on_hover_text(if is_playing {
            "一時停止 [Enter]"
        } else {
            "再生 [Enter]"
        });
        if play_resp.clicked()
            && let Some(p) = self.fs_video_player(fs_idx)
        {
            p.toggle_play();
        }

        // ── ループ再生 トグルボタン (L キーと同等) ──
        let loop_on = self.settings.video_loop;
        let loop_rect = egui::Rect::from_min_size(
            egui::pos2(play_rect.max.x + pad, cy - btn_size / 2.0),
            egui::vec2(btn_size, btn_size),
        );
        let loop_resp = ui.interact(
            loop_rect,
            egui::Id::new(("video_loop", fs_idx)),
            egui::Sense::click(),
        );
        // ループ ON のときはアクセント色 (緑) で背景塗り、ホバーは色濃く。
        let loop_bg = if loop_on {
            if loop_resp.hovered() {
                egui::Color32::from_rgba_unmultiplied(60, 130, 60, 220)
            } else {
                egui::Color32::from_rgba_unmultiplied(50, 100, 50, 180)
            }
        } else if loop_resp.hovered() {
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 40)
        } else {
            egui::Color32::TRANSPARENT
        };
        painter.rect_filled(loop_rect, 4.0, loop_bg);
        let loop_color = if loop_on {
            egui::Color32::from_rgb(180, 240, 180)
        } else {
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 200)
        };
        draw_loop_icon(&painter, loop_rect.center(), btn_size * 0.32, loop_color);
        let loop_resp = loop_resp.on_hover_text(if loop_on {
            "ループ再生 ON (クリックで OFF) [L]"
        } else {
            "ループ再生 OFF (クリックで ON) [L]"
        });
        if loop_resp.clicked() {
            self.settings.video_loop = !self.settings.video_loop;
            self.settings.save();
            if let Some(p) = self.fs_video_player(fs_idx) {
                p.set_loop_enabled(self.settings.video_loop);
            }
        }

        let mut frame_action_x = loop_rect.max.x + pad;
        let mut video_frame_step_request = None;
        let prev_frame_rect = egui::Rect::from_min_size(
            egui::pos2(frame_action_x, cy - btn_size / 2.0),
            egui::vec2(btn_size, btn_size),
        );
        let prev_frame_resp = ui.interact(
            prev_frame_rect,
            egui::Id::new(("video_prev_frame", fs_idx)),
            egui::Sense::click(),
        );
        draw_hud_button_bg(&painter, prev_frame_rect, prev_frame_resp.hovered());
        draw_frame_step_icon(&painter, prev_frame_rect.center(), btn_size * 0.28, -1);
        let prev_frame_resp = prev_frame_resp.on_hover_text("前のフレーム [Ctrl+Shift+←]");
        let prev_down = handle_frame_step_button(
            ui.ctx(),
            fs_idx,
            -1,
            &prev_frame_resp,
            &mut self.video_frame_step_hold,
            &mut video_frame_step_request,
        );
        frame_action_x = prev_frame_rect.max.x + pad;

        let screenshot_rect = egui::Rect::from_min_size(
            egui::pos2(frame_action_x, cy - btn_size / 2.0),
            egui::vec2(btn_size, btn_size),
        );
        let screenshot_resp = ui.interact(
            screenshot_rect,
            egui::Id::new(("video_screenshot", fs_idx)),
            egui::Sense::click(),
        );
        draw_hud_button_bg(&painter, screenshot_rect, screenshot_resp.hovered());
        draw_camera_icon(&painter, screenshot_rect.center(), btn_size * 0.28);
        let screenshot_resp = screenshot_resp.on_hover_text("現在フレームをクリップボードにコピー");
        if screenshot_resp.clicked() {
            self.copy_video_frame_to_clipboard(fs_idx);
        }
        frame_action_x = screenshot_rect.max.x + pad;

        let next_frame_rect = egui::Rect::from_min_size(
            egui::pos2(frame_action_x, cy - btn_size / 2.0),
            egui::vec2(btn_size, btn_size),
        );
        let next_frame_resp = ui.interact(
            next_frame_rect,
            egui::Id::new(("video_next_frame", fs_idx)),
            egui::Sense::click(),
        );
        draw_hud_button_bg(&painter, next_frame_rect, next_frame_resp.hovered());
        draw_frame_step_icon(&painter, next_frame_rect.center(), btn_size * 0.28, 1);
        let next_frame_resp = next_frame_resp.on_hover_text("次のフレーム [Ctrl+Shift+→]");
        let next_down = handle_frame_step_button(
            ui.ctx(),
            fs_idx,
            1,
            &next_frame_resp,
            &mut self.video_frame_step_hold,
            &mut video_frame_step_request,
        );
        frame_action_x = next_frame_rect.max.x + pad;

        if let Some(direction) = video_frame_step_request {
            self.step_video_frame(ui.ctx(), fs_idx, direction);
        }
        if !prev_down
            && !next_down
            && self
                .video_frame_step_hold
                .is_some_and(|h| h.fs_idx == fs_idx)
        {
            self.video_frame_step_hold = None;
        }

        // ── 時刻表示 / 右側コントロール rect ──
        let time_text = format!(
            "{} / {}",
            crate::ui_helpers::format_hms(position),
            crate::ui_helpers::format_hms(duration.max(position)),
        );
        let time_galley = painter.layout_no_wrap(time_text, time_font, egui::Color32::WHITE);
        let time_w = time_galley.size().x;
        let time_h = time_galley.size().y;

        // ── 音量 % (右端、レイアウトのみ — テキストは最後に上に重ねて描く) ──
        // 幅は常に "100%" 分を予約し、% 桁数変化で隣接ウィジェットが震えないようにする。
        // VOL_PCT_MAX_W は 13pt の "100%" 実測幅 (~26-28px、日本語プロポーショナル
        // フォント込みの幅変動を見越して余裕を取った値)。これより広い幅になっても
        // 右側にはみ出すだけで隣接ウィジェットには影響しないが、これより狭いと
        // 99% などで右にずれて見えるので保守的に大きめにする。
        const VOL_PCT_MAX_W: f32 = 36.0;
        let volume = crate::settings::clamp_video_volume(volume);
        let vol_pct_text = format!("{:>3}%", (volume * 100.0).round() as i32);
        let vol_text_color = if volume > 1.0 {
            egui::Color32::from_rgb(255, 210, 80)
        } else {
            egui::Color32::WHITE
        };
        let vol_pct_galley =
            painter.layout_no_wrap(vol_pct_text, label_font.clone(), vol_text_color);
        let vol_pct_block_x = hud_rect.max.x - pad - VOL_PCT_MAX_W;
        let vol_pct_pos = egui::pos2(
            vol_pct_block_x + (VOL_PCT_MAX_W - vol_pct_galley.size().x), // 右寄せ
            cy - vol_pct_galley.size().y / 2.0,
        );

        // ── 音量スライダー rect (描画 / 入力は下) ──
        let vol_slider_w = 90.0;
        let vol_slider_rect = egui::Rect::from_min_max(
            egui::pos2(vol_pct_block_x - pad - vol_slider_w, cy - 4.0),
            egui::pos2(vol_pct_block_x - pad, cy + 4.0),
        );
        let vol_hit_rect = vol_slider_rect.expand2(egui::vec2(0.0, 10.0));

        // ── ミュートアイコン / 倍速 / 時刻表示 rect (描画 / 入力は下) ──
        let mute_rect = egui::Rect::from_min_size(
            egui::pos2(vol_slider_rect.min.x - pad - btn_size, cy - btn_size / 2.0),
            egui::vec2(btn_size, btn_size),
        );

        let speed_rect = egui::Rect::from_min_size(
            egui::pos2(mute_rect.min.x - pad - btn_size * 1.55, cy - btn_size / 2.0),
            egui::vec2(btn_size * 1.55, btn_size),
        );
        let time_pos = egui::pos2(speed_rect.min.x - pad - time_w, cy - time_h / 2.0);
        painter.galley(time_pos, time_galley, egui::Color32::WHITE);

        // ── シークバー (描画 + 入力) ──
        // hit_rect は HUD 全高に広げてクリックしやすくする。視覚的なバーは細いまま。
        // hover 時は target 位置に縦線 + 上に時刻ラベルを出す。
        let seek_x0 = frame_action_x;
        let seek_x1 = time_pos.x - pad;
        if seek_x1 > seek_x0 + 20.0 {
            let bar_rect = egui::Rect::from_min_max(
                egui::pos2(seek_x0, cy - 4.0),
                egui::pos2(seek_x1, cy + 4.0),
            );
            let hit_rect = egui::Rect::from_min_max(
                egui::pos2(seek_x0, hud_rect.min.y),
                egui::pos2(seek_x1, hud_rect.max.y),
            );
            painter.rect_filled(bar_rect, 2.0, egui::Color32::from_gray(80));
            if duration > 0.0 {
                let progress = (position / duration).clamp(0.0, 1.0) as f32;
                let filled = egui::Rect::from_min_max(
                    bar_rect.min,
                    egui::pos2(bar_rect.min.x + bar_rect.width() * progress, bar_rect.max.y),
                );
                painter.rect_filled(filled, 2.0, egui::Color32::from_rgb(220, 220, 220));
            }

            // バーより少し上下に飛び出した縦線で位置を可視化。色は kind ごと
            // (チャプター=水色 / ブックマーク=黄 / ピン=緑)。クリック判定はシークバー
            // 全体に乗せたままで、マーカーは描画のみ。
            if duration > 0.0 {
                for m in self.collect_video_nav_markers(fs_idx) {
                    let x = bar_rect.min.x
                        + bar_rect.width() * (m.pts / duration).clamp(0.0, 1.0) as f32;
                    painter.line_segment(
                        [
                            egui::pos2(x, bar_rect.min.y - 6.0),
                            egui::pos2(x, bar_rect.max.y + 6.0),
                        ],
                        egui::Stroke::new(2.0, nav_marker_color(m.kind)),
                    );
                }
            }
            let seek_resp = ui.interact(
                hit_rect,
                egui::Id::new(("video_seek", fs_idx)),
                egui::Sense::click_and_drag(),
            );
            if seek_resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
            // hover プレビュー: 縦線 + サムネ画像 + 時刻ラベル
            if seek_resp.hovered()
                && duration > 0.0
                && let Some(hp) = ui.ctx().input(|i| i.pointer.hover_pos())
            {
                let x = hp.x.clamp(bar_rect.min.x, bar_rect.max.x);
                painter.line_segment(
                    [
                        egui::pos2(x, hud_rect.min.y + 4.0),
                        egui::pos2(x, hud_rect.max.y - 4.0),
                    ],
                    egui::Stroke::new(1.5, egui::Color32::from_rgb(255, 220, 120)),
                );
                let frac = ((x - bar_rect.min.x) / bar_rect.width()).clamp(0.0, 1.0) as f64;
                let target = frac * duration;

                // サムネ要求 (毎フレーム呼んで OK、worker 側で drain + LRU)
                if let Some(p) = self.fs_video_player(fs_idx) {
                    p.request_seek_thumbnail(target);
                }

                // 直近キャッシュからサムネ取得 → GPU テクスチャに反映 → 描画
                // Phase 8.H: サムネ未到着時もプレースホルダ枠を出して「ロード後に
                // 枠が出る」ちらつきを防ぐ。サイズは動画 aspect から逆算した固定値で、
                // ロード完了で同じ位置に画像差し替え。
                let thumb_opt = self
                    .fs_video_player(fs_idx)
                    .and_then(|p| p.nearest_seek_thumbnail(target));
                let label = crate::ui_helpers::format_hms(target);
                let galley =
                    painter.layout_no_wrap(label, label_font.clone(), egui::Color32::WHITE);
                let label_size = galley.size();
                const GAP_THUMB_TO_HUD: f32 = 8.0;
                const GAP_THUMB_TO_LABEL: f32 = 4.0;
                const LABEL_PAD: f32 = 4.0;
                // サムネ既定サイズ (= ロード前後で位置・寸法を一致させる)。
                // ワーカーは THUMB_W x THUMB_H 上限で aspect-fit するので同じ計算を
                // ここでも行い、ロード後のサムネと完全に同じ寸法でプレースホルダを
                // 出す (= ちらつき防止)。動画 info が無い場合は 16:9 で代替。
                let (src_w, src_h) = self
                    .fs_video_player(fs_idx)
                    .and_then(|p| p.info())
                    .map(|i| (i.width.max(1), i.height.max(1)))
                    .unwrap_or((1280, 720));
                let (placeholder_w, placeholder_h) = {
                    let max_w = crate::video::thumbnail::THUMB_W as f64;
                    let max_h = crate::video::thumbnail::THUMB_H as f64;
                    let scale = (max_w / src_w as f64).min(max_h / src_h as f64);
                    let w = ((src_w as f64 * scale).round() as f32).max(1.0);
                    let h = ((src_h as f64 * scale).round() as f32).max(1.0);
                    (w, h)
                };
                let (thumb_w, thumb_h) = match thumb_opt.as_ref() {
                    Some(t) => (t.width as f32, t.height as f32),
                    None => (placeholder_w, placeholder_h),
                };
                let thumb_x = (x - thumb_w / 2.0)
                    .clamp(full_rect.min.x + 4.0, full_rect.max.x - thumb_w - 4.0);
                let label_block_h = label_size.y + LABEL_PAD * 2.0;
                let mut thumb_y = hud_rect.min.y
                    - GAP_THUMB_TO_HUD
                    - label_block_h
                    - GAP_THUMB_TO_LABEL
                    - thumb_h;
                let min_y = full_rect.min.y + 4.0;
                if thumb_y < min_y {
                    thumb_y = min_y;
                }
                let thumb_rect = egui::Rect::from_min_size(
                    egui::pos2(thumb_x, thumb_y),
                    egui::vec2(thumb_w, thumb_h),
                );
                // 共通: 外側の黒背景 (= 枠の外側 padding) + 中身は黒で塗っておく。
                painter.rect_filled(
                    thumb_rect.expand(2.0),
                    3.0,
                    egui::Color32::from_rgba_unmultiplied(0, 0, 0, 220),
                );
                painter.rect_filled(thumb_rect, 2.0, egui::Color32::BLACK);
                if let Some(thumb) = thumb_opt.as_ref() {
                    let tex_id = self.upload_seek_thumb_texture(ui.ctx(), fs_idx, thumb);
                    painter.image(
                        tex_id,
                        thumb_rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                } else {
                    // 読み込み中表示 (= サムネ未到着、worker が抽出中)。
                    let loading = "読込中...";
                    let loading_galley = painter.layout_no_wrap(
                        loading.to_string(),
                        egui::FontId::proportional(12.0),
                        egui::Color32::from_rgb(200, 200, 200),
                    );
                    let loading_pos = egui::pos2(
                        thumb_rect.center().x - loading_galley.size().x / 2.0,
                        thumb_rect.center().y - loading_galley.size().y / 2.0,
                    );
                    painter.galley(
                        loading_pos,
                        loading_galley,
                        egui::Color32::from_rgb(200, 200, 200),
                    );
                }
                painter.rect_stroke(
                    thumb_rect,
                    2.0,
                    egui::Stroke::new(1.0, egui::Color32::from_gray(180)),
                    egui::StrokeKind::Inside,
                );

                // 時刻ラベル: サムネ枠の **直下** に常に独立行で表示。
                let label_pos = egui::pos2(
                    (x - label_size.x / 2.0).clamp(
                        full_rect.min.x + LABEL_PAD,
                        full_rect.max.x - label_size.x - LABEL_PAD,
                    ),
                    thumb_rect.max.y + GAP_THUMB_TO_LABEL + LABEL_PAD,
                );
                let bg = egui::Rect::from_min_size(label_pos, label_size).expand(LABEL_PAD);
                painter.rect_filled(bg, 3.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 200));
                painter.galley(label_pos, galley, egui::Color32::WHITE);
            }
            if (seek_resp.clicked() || seek_resp.dragged())
                && duration > 0.0
                && let Some(pp) = seek_resp.interact_pointer_pos()
                && let Some(p) = self.fs_video_player(fs_idx)
            {
                let frac = ((pp.x - bar_rect.min.x) / bar_rect.width()).clamp(0.0, 1.0);
                p.seek(frac as f64 * duration);
            }
        }

        // ── ミュートボタン (描画 + 入力) ──
        let mute_resp = ui.interact(
            mute_rect,
            egui::Id::new(("video_mute", fs_idx)),
            egui::Sense::click(),
        );
        draw_hud_button_bg(&painter, mute_rect, mute_resp.hovered());
        draw_speaker_icon(&painter, mute_rect.center(), btn_size * 0.55, muted);
        let mute_resp = mute_resp.on_hover_text(if muted {
            "ミュート解除 [M]"
        } else {
            "ミュート [M]"
        });
        if mute_resp.clicked()
            && let Some(p) = self.fs_video_player(fs_idx)
        {
            p.set_muted(!muted);
        }

        // ── 倍速ボタン + ポップアップ ──
        let speed_resp = ui.interact(
            speed_rect,
            egui::Id::new(("video_speed", fs_idx)),
            egui::Sense::click(),
        );
        draw_hud_button_bg(&painter, speed_rect, speed_resp.hovered());
        painter.text(
            speed_rect.center(),
            egui::Align2::CENTER_CENTER,
            crate::video::clock::format_playback_speed(playback_speed),
            egui::FontId::proportional(12.0),
            egui::Color32::WHITE,
        );
        let speed_resp = speed_resp.on_hover_text("再生速度");
        if speed_resp.clicked() {
            self.video_speed_popup_open = !self.video_speed_popup_open;
            self.video_hud_last_activity = Some(std::time::Instant::now());
        }
        if self.video_speed_popup_open {
            let popup_w = 356.0_f32.min((full_rect.width() - 16.0).max(180.0));
            let popup_h = 74.0;
            let popup_x = (speed_rect.center().x - popup_w * 0.5)
                .clamp(full_rect.min.x + 8.0, full_rect.max.x - popup_w - 8.0);
            let popup_y = (hud_rect.min.y - popup_h - 6.0).max(full_rect.min.y + 8.0);
            let mut selected_speed = None;
            egui::Area::new(egui::Id::new(("video_speed_popup", fs_idx)))
                .order(egui::Order::Foreground)
                .fixed_pos(egui::pos2(popup_x, popup_y))
                .show(ui.ctx(), |ui| {
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgba_unmultiplied(0, 0, 0, 225))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(110)))
                        .corner_radius(egui::CornerRadius::same(4))
                        .inner_margin(egui::Margin::same(6))
                        .show(ui, |ui| {
                            ui.set_min_width(popup_w - 12.0);
                            ui.horizontal_wrapped(|ui| {
                                for speed in crate::video::clock::PLAYBACK_SPEED_CHOICES {
                                    let selected = (playback_speed - speed).abs() < 1.0e-6;
                                    let label = crate::video::clock::format_playback_speed(speed);
                                    let button = egui::Button::new(label)
                                        .selected(selected)
                                        .min_size(egui::vec2(46.0, 24.0));
                                    if ui.add(button).clicked() {
                                        selected_speed = Some(speed);
                                    }
                                }
                            });
                        });
                });
            if ui.ctx().input(|i| i.pointer.any_click())
                && !speed_resp.hovered()
                && let Some(pos) = ui.ctx().input(|i| i.pointer.interact_pos())
            {
                let popup_rect = egui::Rect::from_min_size(
                    egui::pos2(popup_x, popup_y),
                    egui::vec2(popup_w, popup_h),
                );
                if !popup_rect.contains(pos) {
                    self.video_speed_popup_open = false;
                }
            }
            if let Some(speed) = selected_speed {
                let speed = crate::video::clock::clamp_playback_speed(speed);
                let speed_changed = (self.video_playback_speed - speed).abs() > 1.0e-9;
                self.video_playback_speed = speed;
                self.video_speed_popup_open = false;
                if let Some(p) = self.fs_video_player(fs_idx) {
                    p.set_playback_speed(speed);
                }
                if speed_changed {
                    // Y 軸スケールが追従するため、旧スケールのサンプルを残すと
                    // 表示色が不整合になる。クリアして新スケールで再構築する。
                    self.reset_video_perf_history();
                }
            }
        }

        // ── 音量スライダー (描画 + 入力) ──
        painter.rect_filled(vol_slider_rect, 2.0, egui::Color32::from_gray(80));
        let max_volume = crate::settings::VIDEO_VOLUME_MAX;
        let normal_frac = (1.0 / max_volume) as f32;
        let normal_fill_frac = (volume.min(1.0) / max_volume) as f32;
        if normal_fill_frac > 0.0 {
            let normal_fill = egui::Rect::from_min_max(
                vol_slider_rect.min,
                egui::pos2(
                    vol_slider_rect.min.x + vol_slider_rect.width() * normal_fill_frac,
                    vol_slider_rect.max.y,
                ),
            );
            painter.rect_filled(normal_fill, 2.0, egui::Color32::from_rgb(220, 220, 220));
        }
        if volume > 1.0 {
            let boost_fill_frac = (volume / max_volume) as f32;
            let boost_fill = egui::Rect::from_min_max(
                egui::pos2(
                    vol_slider_rect.min.x + vol_slider_rect.width() * normal_frac,
                    vol_slider_rect.min.y,
                ),
                egui::pos2(
                    vol_slider_rect.min.x + vol_slider_rect.width() * boost_fill_frac,
                    vol_slider_rect.max.y,
                ),
            );
            painter.rect_filled(boost_fill, 2.0, egui::Color32::from_rgb(255, 198, 62));
        }
        let normal_x = vol_slider_rect.min.x + vol_slider_rect.width() * normal_frac;
        painter.line_segment(
            [
                egui::pos2(normal_x, vol_slider_rect.min.y - 3.0),
                egui::pos2(normal_x, vol_slider_rect.max.y + 3.0),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_gray(150)),
        );
        let vol_resp = ui.interact(
            vol_hit_rect,
            egui::Id::new(("video_vol", fs_idx)),
            egui::Sense::click_and_drag(),
        );
        if vol_resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        let vol_resp =
            vol_resp.on_hover_text("音量 (右クリック / ダブルクリックで 100%) [Shift+↑ / Shift+↓]");
        if vol_resp.secondary_clicked() || vol_resp.double_clicked() {
            if let Some(p) = self.fs_video_player(fs_idx) {
                p.set_volume(1.0);
            }
            self.settings.video_volume = 1.0;
            self.settings.save();
        } else if (vol_resp.clicked() || vol_resp.dragged())
            && let Some(pp) = vol_resp.interact_pointer_pos()
        {
            let frac = ((pp.x - vol_slider_rect.min.x) / vol_slider_rect.width()).clamp(0.0, 1.0)
                as f64
                * max_volume;
            if let Some(p) = self.fs_video_player(fs_idx) {
                p.set_volume(frac);
            }
            // Phase 8 (Codex P3-1): 即時 save (HUD スライダー操作の永続化)。
            self.settings.video_volume = crate::settings::clamp_video_volume(frac);
            self.settings.save();
        }

        // ── 音量 % テキスト (一番最後に描画 = 上に乗せる) ──
        painter.galley(vol_pct_pos, vol_pct_galley, vol_text_color);
    }

    /// 動画一時停止中のキー操作ヒントを **画面中央の再生アイコン直下** に表示する
    /// (Phase 6 改修: 旧 HUD 直上から、再生アイコン直下に移動。一時停止中の
    /// 56px 半径再生アイコンが画面中央に出ているので、その下にヒントを置くことで
    /// 「Enter を押せば再生」の関連付けが瞬時に分かるようにする)。
    ///
    /// 表示条件 (Codex Phase 5.1 P2 反映):
    /// - エラー無し
    /// - メタデータ取得済 (= info().is_some()、Loading 中は隠す)
    /// - 再生中ではない
    /// - 末尾近くではない (= EOF 状態では Enter は seek-to-0 になるため、ここで
    ///   「再生開始」と書くと挙動と齟齬。loop 設定オフで末尾停止しているケースを除外)
    pub(crate) fn draw_video_paused_hint(
        &self,
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
        fs_idx: usize,
    ) {
        let show_hint = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => {
                if player.error().is_some() {
                    false
                } else if player.info().is_none() {
                    false
                } else if player.is_playing() {
                    false
                } else if player.is_frame_step_active() {
                    false
                } else {
                    let dur = player.duration();
                    let pos = player.position();
                    !(dur > 0.0 && pos >= dur - 0.5)
                }
            }
            _ => false,
        };
        if !show_hint {
            return;
        }
        let painter = ui.painter();
        let line1 = "[Enter] 再生開始";
        let line2 = "[Shift]+[Enter] 外部プレイヤーで再生";
        let font = egui::FontId::proportional(16.0);
        let text_color = egui::Color32::from_rgba_unmultiplied(230, 230, 230, 230);
        let g1 = painter.layout_no_wrap(line1.to_string(), font.clone(), text_color);
        let g2 = painter.layout_no_wrap(line2.to_string(), font.clone(), text_color);
        let line_gap = 4.0;
        let block_w = g1.size().x.max(g2.size().x);
        let block_h = g1.size().y + line_gap + g2.size().y;
        let pad = 10.0;
        // 中央 2 ボタン (= 半径 56、ラベル "最初から" / "続きから" 込み) の直下に配置。
        // ボタン center.y からアイコン半径 56 + ラベル 14px 余白 + ヒント余白 28px = 98px。
        let center_x = full_rect.center().x;
        let top_y = full_rect.center().y + 98.0;
        let bg_rect = egui::Rect::from_min_max(
            egui::pos2(center_x - block_w / 2.0 - pad, top_y),
            egui::pos2(center_x + block_w / 2.0 + pad, top_y + block_h + pad * 2.0),
        );
        painter.rect_filled(
            bg_rect,
            6.0,
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 184),
        );
        painter.rect_stroke(
            bg_rect,
            6.0,
            egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 38),
            ),
            egui::StrokeKind::Outside,
        );
        let l1_pos = egui::pos2(center_x - g1.size().x / 2.0, bg_rect.min.y + pad);
        painter.galley(l1_pos, g1, text_color);
        let l2_pos = egui::pos2(
            center_x - g2.size().x / 2.0,
            bg_rect.min.y + pad + (block_h - g2.size().y),
        );
        painter.galley(l2_pos, g2, text_color);
    }
}

/// 動画 HUD の下部バー領域 (高さ 44px)。
/// `handle_fs_wheel_and_click` でこの矩形内のクリックを toggle_play から除外するために
/// 共通化している。
pub(crate) fn video_hud_rect(full_rect: egui::Rect) -> egui::Rect {
    let hud_h = 44.0;
    egui::Rect::from_min_max(
        egui::pos2(full_rect.min.x, full_rect.max.y - hud_h),
        full_rect.max,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid_item::GridItem;
    use std::path::PathBuf;

    #[test]
    fn location_display_regular_image_joins_folder_and_filename() {
        let out = compute_location_display(
            Some(&GridItem::Image(PathBuf::from(r"C:\photos\2024\img.jpg"))),
            r"C:\photos\2024",
            "img.jpg",
        );
        assert_eq!(out, r"C:\photos\2024\img.jpg");
    }

    #[test]
    fn location_display_regular_image_handles_trailing_separator() {
        // ドライブルート直下 "C:\" のような末尾 '\' ケース
        let out = compute_location_display(
            Some(&GridItem::Image(PathBuf::from(r"C:\img.jpg"))),
            r"C:\",
            "img.jpg",
        );
        assert_eq!(out, r"C:\img.jpg");
    }

    #[test]
    fn location_display_zip_image_uses_arrow_separator() {
        let out = compute_location_display(
            Some(&GridItem::ZipImage {
                zip_path: PathBuf::from(r"C:\archives\book.zip"),
                entry_name: "ch01/page01.jpg".to_string(),
            }),
            r"C:\archives\book.zip",
            "page01.jpg",
        );
        assert_eq!(out, r"C:\archives\book.zip > ch01/page01.jpg");
    }

    #[test]
    fn location_display_pdf_page_shows_page_number() {
        let out = compute_location_display(
            Some(&GridItem::PdfPage {
                pdf_path: PathBuf::from(r"C:\docs\manual.pdf"),
                page_num: 4, // 0-indexed なので表示は "Page 5"
                content_type: None,
            }),
            r"C:\docs\manual.pdf",
            "Page 5",
        );
        assert_eq!(out, r"C:\docs\manual.pdf > Page 5");
    }

    /// 変換済み 7z/LZH を閲覧中は `effective_folder()` が元アーカイブのパスを
    /// 返す想定。`base_folder` にその値が渡ってくるので、キャッシュ ZIP のパス
    /// ではなく元 7z/LZH が表示される。
    #[test]
    fn location_display_uses_override_path_for_converted_archives() {
        let out = compute_location_display(
            Some(&GridItem::ZipImage {
                // zip_path はキャッシュ ZIP 側 (UI 表示には使わない)
                zip_path: PathBuf::from(r"C:\AppData\cache\abc\book.zip"),
                entry_name: "page01.jpg".to_string(),
            }),
            // base_folder は effective_folder() → 元 LZH
            r"C:\downloads\book.lzh",
            "page01.jpg",
        );
        assert_eq!(out, r"C:\downloads\book.lzh > page01.jpg");
    }

    #[test]
    fn location_display_empty_base_falls_back_to_filename() {
        let out = compute_location_display(
            Some(&GridItem::Image(PathBuf::from("img.jpg"))),
            "",
            "img.jpg",
        );
        assert_eq!(out, "img.jpg");
    }

    #[test]
    fn location_display_empty_base_zip_falls_back_to_entry() {
        let out = compute_location_display(
            Some(&GridItem::ZipImage {
                zip_path: PathBuf::from("book.zip"),
                entry_name: "page01.jpg".to_string(),
            }),
            "",
            "page01.jpg",
        );
        assert_eq!(out, "page01.jpg");
    }

    // ── build_info_text: ダウンスケール警告 ─────────────────────────

    #[test]
    fn info_text_dims_only_no_marker_when_not_downscaled() {
        let s = build_info_text(Some((1920, 1080)), None, false, None, None);
        assert_eq!(s, "1920 × 1080");
    }

    #[test]
    fn info_text_appends_downscale_marker_when_flag_set() {
        let s = build_info_text(Some((7168, 9216)), None, true, None, None);
        assert!(s.starts_with("7168 × 9216"));
        assert!(
            s.contains("ダウンスケール表示中"),
            "warning marker present: {s:?}"
        );
    }

    #[test]
    fn info_text_marker_comes_after_ai_info() {
        // AI 情報 "(漫画 ...)" の内側に警告が混入しないこと。
        let s = build_info_text(
            Some((7168, 9216)),
            None,
            true,
            Some(("漫画", 28672, 36864)),
            None,
        );
        let ai_end = s.find(')').expect("AI closing paren exists");
        let marker_pos = s.find("ダウンスケール").expect("marker present");
        assert!(
            marker_pos > ai_end,
            "marker should come after AI info: {s:?}"
        );
    }

    #[test]
    fn info_text_no_marker_when_dims_missing() {
        // dims が None のとき (まだロード中) は downscaled=true でも警告を出さない。
        let s = build_info_text(None, Some(1_234_567), true, None, None);
        assert!(
            !s.contains("ダウンスケール"),
            "no marker without dims: {s:?}"
        );
    }
}
