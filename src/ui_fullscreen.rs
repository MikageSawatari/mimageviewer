//! フルスクリーン表示のレンダリング。
//!
//! `App::update()` から呼ばれる `render_fullscreen_viewport()` を実装する。
//! 元は `update()` 内にインラインで書かれていた ~460 行を独立メソッドに切り出したもの。
//!
//! # ⚠️ 動画 UI の責任分担に注意
//!
//! - **動画フルスクリーン中の HUD・音量スライダ・seek bar・ミュート・top bar・
//!   メタデータパネル・ノーマライズボタン等の playback controls は、このファイルではなく
//!   [`crate::video::native_presenter`] (egui overlay) が描画する**
//! - このファイルが現役で担当する動画関連は:
//!   - 動画の **エラー表示** と **「動画を準備中…」スピナー**
//!     ([`crate::App::draw_video_hud`])
//! - 動画 UI を変更したい場合は必ず [docs/video-architecture.md] と
//!   [src/video/native_presenter/mod.rs] を読むこと
//!
//! 過去にこのファイル内 (line 6800 付近) に動画 HUD コードが書かれていた経緯があるが、
//! v0.9.0 で native presenter に移行済。新規追加 / 移植時に「ui_fullscreen.rs を見て
//! しまう」誤認を避けるため明示。

use eframe::egui;
use std::sync::Arc;

use crate::app::App;
use crate::fs_animation::FsCacheEntry;
use crate::grid_item::{GridItem, ThumbnailState};
use crate::pdf_loader::PdfPageContentType;
use crate::settings::SpreadMode;
use crate::ui_helpers::{HoverTipExt, open_external_player};

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

#[derive(Clone, Copy)]
enum ComparePreparedTextureKind {
    Pinned,
    Current,
    Diff,
}

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
    let report = crate::video::native_window::claim_foreground(target_hwnd as u64);
    NativeFocusClaim {
        foreground_hwnd: report.foreground_hwnd as usize,
        post_foreground_hwnd: report.post_foreground_hwnd as usize,
        target_hwnd: report.target_hwnd as usize,
        set_foreground_ok: report.set_foreground_ok,
        attach_thread_input_ok: report.attach_thread_input_ok,
        set_active_ok: report.set_active_ok,
        set_focus_ok: report.set_focus_ok,
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
            | egui::Key::C
            | egui::Key::P
            | egui::Key::T
            | egui::Key::I
            | egui::Key::X
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
        egui::Key::C => 0x43,
        egui::Key::J => 0x4A,
        egui::Key::K => 0x4B,
        egui::Key::L => 0x4C,
        egui::Key::M => 0x4D,
        egui::Key::P => 0x50,
        egui::Key::S => 0x53,
        egui::Key::W => 0x57,
        egui::Key::X => 0x58,
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

/// 補正ショートカット (U/T/N) のスコープ。どの層を書き換えるかを表す。
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
/// フィードバックトースト表示時間（秒）。短い確認系トーストの既定値。
/// 複数行の案内文は `show_feedback_toast_with_duration` で長めを指定する。
pub(crate) const FEEDBACK_TOAST_DURATION: f32 = 1.2;
/// 境界ヒント（最初/最後の項目に達した案内）の表示時間（秒）
const BOUNDARY_HINT_DURATION: f32 = 2.5;
/// 画像・動画フォルダが見つからない旨のヒント表示時間（秒）。メッセージが長く
/// ユーザーがフルスクリーンを維持するか Esc で抜けるか判断する時間が要るため、
/// 境界ヒントより長めに取る。
const NO_IMAGE_FOLDER_HINT_DURATION: f32 = 4.0;

/// J/K でマーカー間ジャンプするときの「現在位置とみなす許容幅」(秒)。
/// 現在位置とほぼ同じマーカーをスキップして次のマーカーへ進めるための余裕。
const NAV_MARKER_EPSILON: f64 = 0.5;

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

/// フルスクリーン中央のヒントオーバーレイ。
#[derive(Copy, Clone)]
pub(crate) enum FsBoundaryHint {
    /// 最初/最後の項目に到達 (at_end: true=末尾, false=先頭)。
    Edge {
        at_end: bool,
        at: std::time::Instant,
    },
    /// Ctrl+↑↓ で画像・動画のある次 (forward=true) / 前 (forward=false) のフォルダが
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
    /// Ctrl+F active / 検索結果一覧など、現在の scope では Ctrl+↑↓ が移動を
    /// 開始しないことを知らせる。
    NavNoOp {
        reason: FsNavNoOpReason,
        at: std::time::Instant,
    },
}

#[derive(Copy, Clone)]
pub(crate) enum FsNavNoOpReason {
    LocalFilterActive,
    SearchResultList,
}

impl FsBoundaryHint {
    pub(crate) fn started_at(&self) -> std::time::Instant {
        match self {
            FsBoundaryHint::Edge { at, .. }
            | FsBoundaryHint::NoImageFolder { at, .. }
            | FsBoundaryHint::SearchEnd { at, .. }
            | FsBoundaryHint::NavNoOp { at, .. } => *at,
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
    /// (DFS が境界に当たって path=None / 画像・動画フォルダに着地できず !hit_image_folder /
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

    /// フルスクリーンビューポートのライフサイクル後始末を行う。
    ///
    /// - フルスクリーンが現在アクティブ (`fullscreen_idx.is_some()`) なら何もしない
    ///   (描画は `render_fullscreen_viewport` が担当)。
    /// - PDF 列挙待ちの遷移中は holdover を表示してちらつきを防ぐ。
    /// - フルスクリーン終了直後の 1 フレームだけ `Visible(false)` を送って hidden に落とす。
    /// - それ以外のアイドル時は何もしない。以前は「入場時のちらつき防止」のため毎フレーム
    ///   `show_viewport_immediate(...with_visible(false), ...)` を呼んでいたが、hidden viewport の
    ///   常時維持が `fullscreen_viewport_ms` の主要候補だった (2026-05-10 perf log で
    ///   非アクティブ時に 30-70ms/frame を計測、内訳分割は同改修で追加) ため、呼ばない方針に
    ///   変更した。代償として `close_fullscreen` 後の再入場時に 1x1 → フルサイズの DWM 遷移
    ///   フラッシュが毎回出る (`fs_viewport_recreate_after_hide` で generation が進み新しい
    ///   ViewportId になるため)。
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

        // フルスクリーン非アクティブ時の hidden viewport 維持コストを排除する。
        // 詳細は関数 doc を参照。
        if !self.fs_viewport_shown {
            return;
        }
        // ここに来るのは close_fullscreen 直後の 1 フレーム。
        // show_viewport_immediate を 1 回呼んで viewport を alive にし、
        // ViewportBuilder::with_visible(false) は「initial」可視性しか制御しないため、
        // 一度表示済みのビューポートを隠すには明示的に Visible(false) を送る必要がある。
        // 送信直前に DWM トランジションを無効化して Win11 のフェードアウトを抑止する。
        let fs_builder = self.build_fullscreen_viewport_builder().with_visible(false);
        ctx.show_viewport_immediate(fs_id, fs_builder, |_ctx, _class| {});
        crate::dwm_transitions::disable_transitions_for_thread_windows();
        ctx.send_viewport_cmd_to(fs_id, egui::ViewportCommand::Visible(false));
        self.fs_viewport_shown = false;
        if self.fs_viewport_recreate_after_hide {
            self.fs_viewport_generation = self.fs_viewport_generation.wrapping_add(1);
            self.fs_viewport_recreate_after_hide = false;
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
        // 動画フルスクリーンの黒 backdrop は **presenter HWND が未確定の起動中だけ**
        // 出す純粋な「起動カバー」。fresh open で presenter が立ち上がるまでの黒画面を
        // 隠すのが唯一の役目で、popup HWND が確定したら隠して viewport を破棄させる
        // (presenter popup 自身が黒背景 DComp visual を持つので backdrop は不要)。
        //   - in-window モード: presenter child が main のクライアント領域に直接
        //     描画するので backdrop は出さない (出すと別 top-level window が
        //     foreground を奪い画面全体が黒くなる)。
        //   - Plan B のウィンドウ / 全画面トグル: popup は常に HWND 確定済み
        //     (`hwnd_ready`) なので backdrop は一切出さない。これにより backdrop の
        //     破棄・再生成 (= 白フラッシュ / 黒被り) がトグルで起きない。
        #[cfg(windows)]
        if self.native_video_backdrop_target_for_fs(fs_idx) {
            let hwnd_ready = self.native_video_presenter_hwnd_for_fs(fs_idx).is_some();
            let startup_cover = !self.native_video_in_window_active
                && !hwnd_ready
                && self.native_video_presenter_pending_for_fs(fs_idx);
            if startup_cover {
                self.show_native_video_black_backdrop(ctx, fs_idx);
            } else {
                self.hide_native_video_black_backdrop_if_shown(ctx);
            }
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
        // in-window モード中は静止画を専用 viewport ではなくメインウィンドウの
        // egui ctx に直接描画する (embedded)。本関数冒頭で動画は early-return
        // 済みなので、ここに来る fs_idx は常に非動画。
        #[cfg(windows)]
        let embedded = self.fullscreen_embedded_still_active();
        #[cfg(not(windows))]
        let embedded = false;
        let main_ctx = ctx;
        #[cfg(windows)]
        if embedded && self.fs_viewport_shown {
            // 万一 viewport モードから embedded へ切り替わったら、残った
            // フルスクリーン viewport を隠してから embedded 描画へ移る。
            self.hide_native_video_black_backdrop_if_shown(ctx);
        }
        let fs_builder = self.build_fullscreen_viewport_builder();
        let fs_id = self.fullscreen_viewport_id();
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

        {
            let mut render_fs_body = |ctx: &egui::Context, embedded: bool| {
                let closure_t0 = std::time::Instant::now();
                let setup_t0 = std::time::Instant::now();
                // フルスクリーンビューポート内のイベントで IME 状態を更新する
                // (メインビューポートとは別のイベントキューなのでここで呼ぶ必要がある)。
                // embedded のときは ctx = main ctx で、IME 状態は update() 冒頭で
                // 既に更新済みなので二重処理しない。
                if !embedded {
                    self.update_ime_state(ctx);
                }
                if need_show && !embedded {
                    // embedded のときは専用 viewport を作らないので Visible/Focus は
                    // 送らない (main ウィンドウは既に表示・フォーカス済み)。
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    ctx.send_viewport_cmd(egui::ViewportCommand::CursorVisible(true));
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
                    if self.cursor_hidden {
                        ctx.send_viewport_cmd(egui::ViewportCommand::CursorVisible(true));
                    }
                    self.cursor_hidden = false;
                }

                // event consume される前に捕捉 (handle_fs_key_input が矢印等を
                // 消費するとイベントが見えなくなるため)。マウス移動は操作と見なさない。
                if self.fs_boundary_hint.is_some() {
                    had_user_input_in_frame = ctx.input(|i| {
                        i.events.iter().any(|e| {
                            matches!(
                                e,
                                egui::Event::Key { pressed: true, .. }
                                    | egui::Event::PointerButton { pressed: true, .. }
                                    | egui::Event::MouseWheel { .. }
                            )
                        })
                    });
                }

                if !embedded && ctx.input(|i| i.viewport().close_requested()) {
                    // embedded のときの close_requested は main ウィンドウの × =
                    // アプリ終了要求。フルスクリーン解除ではないので拾わない。
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
                        let compare_wipe_active = matches!(
                            self.compare_view_mode,
                            crate::app::CompareViewMode::Wipe { .. }
                        );
                        let analysis_active = self.analysis_mode && !is_spread_double;
                        // 補正パネルは見開き Double でも使えるようにする (左右独立補正 + コピー)。
                        // 編集対象 (画面上の左/右) は `adjust_spread_target` で切替。
                        let adjustment_active = self.adjustment_mode && !compare_wipe_active;
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
                                    let compare_mode = self.compare_view_mode;
                                    let compare_requested = !matches!(
                                        compare_mode,
                                        crate::app::CompareViewMode::Off
                                    );
                                    if compare_requested {
                                        self.ensure_compare_prepared_pair(ctx, fs_idx);
                                    }
                                    if compare_requested
                                        && self.draw_compare_prepared_mode(
                                            ui,
                                            ctx,
                                            image_rect,
                                            compare_mode,
                                            zp,
                                        )
                                    {
                                        // 比較表示側で描画済み。
                                    } else if matches!(
                                        compare_mode,
                                        crate::app::CompareViewMode::PinnedNormal
                                    ) {
                                        let compare_tex = self.ensure_compare_pinned_texture(ctx);
                                        if let Some(tex) = compare_tex.as_ref() {
                                            let bg_style = self.fs_bg_style(ctx);
                                            Self::draw_compare_pinned_image(
                                                ui, image_rect, tex, zp, &bg_style, None,
                                            );
                                        }
                                    } else {
                                        let fallback_compare_tex = if matches!(
                                            compare_mode,
                                            crate::app::CompareViewMode::Wipe { .. }
                                        ) {
                                            self.ensure_compare_pinned_texture(ctx)
                                        } else {
                                            None
                                        };
                                        let bg_style = self.fs_bg_style(ctx);
                                        Self::draw_fs_image(
                                            ui, image_rect,
                                            state.tex.as_ref(), state.thumb_tex.as_ref(),
                                            state.is_video, state.vst3_waiting_for_video,
                                            state.fs_load_failed, fs_rotation, zp,
                                            free_rot, &bg_style, &state.location_display,
                                        );
                                        if let crate::app::CompareViewMode::Wipe { fraction } =
                                            compare_mode
                                        {
                                            if let Some(tex) = fallback_compare_tex.as_ref() {
                                                let wipe_x = image_rect.left()
                                                    + image_rect.width()
                                                        * fraction.clamp(0.05, 0.95);
                                                let clip = egui::Rect::from_min_max(
                                                    image_rect.min,
                                                    egui::pos2(wipe_x, image_rect.max.y),
                                                );
                                                Self::draw_compare_pinned_image(
                                                    ui,
                                                    image_rect,
                                                    tex,
                                                    zp,
                                                    &bg_style,
                                                    Some(clip),
                                                );
                                                Self::draw_compare_wipe_line(
                                                    ui, image_rect, fraction,
                                                );
                                            }
                                        }
                                    }
                                    // 単一表示時は見開きレイアウトキャッシュを破棄
                                    self.fs_spread_layout = None;
                                }
                                SpreadPair::Double { left, right } => {
                                    let compare_mode = self.compare_view_mode;
                                    let zoom_pan = self.fs_zoom_pan();
                                    let compare_requested = !matches!(
                                        compare_mode,
                                        crate::app::CompareViewMode::Off
                                    );
                                    if compare_requested {
                                        self.ensure_compare_prepared_pair(ctx, fs_idx);
                                    }
                                    if compare_requested
                                        && self.draw_compare_prepared_mode(
                                            ui,
                                            ctx,
                                            image_rect,
                                            compare_mode,
                                            zoom_pan,
                                        )
                                    {
                                        self.fs_spread_layout = None;
                                    } else if matches!(
                                        compare_mode,
                                        crate::app::CompareViewMode::PinnedNormal
                                    ) {
                                        let compare_tex = self.ensure_compare_pinned_texture(ctx);
                                        if let Some(tex) = compare_tex.as_ref() {
                                            let bg_style = self.fs_bg_style(ctx);
                                            Self::draw_compare_pinned_image(
                                                ui, image_rect, tex, zoom_pan, &bg_style, None,
                                            );
                                            self.fs_spread_layout = None;
                                        }
                                    } else {
                                        let fallback_compare_tex = if matches!(
                                            compare_mode,
                                            crate::app::CompareViewMode::Wipe { .. }
                                        ) {
                                            self.ensure_compare_pinned_texture(ctx)
                                        } else {
                                            None
                                        };
                                        self.draw_fs_spread(
                                            ui,
                                            ctx,
                                            image_rect,
                                            left,
                                            right,
                                            state.original_preview_active,
                                        );
                                        if let crate::app::CompareViewMode::Wipe { fraction } =
                                            compare_mode
                                        {
                                            if let Some(tex) = fallback_compare_tex.as_ref() {
                                                let bg_style = self.fs_bg_style(ctx);
                                                let wipe_x = image_rect.left()
                                                    + image_rect.width()
                                                        * fraction.clamp(0.05, 0.95);
                                                let clip = egui::Rect::from_min_max(
                                                    image_rect.min,
                                                    egui::pos2(wipe_x, image_rect.max.y),
                                                );
                                                Self::draw_compare_pinned_image(
                                                    ui,
                                                    image_rect,
                                                    tex,
                                                    zoom_pan,
                                                    &bg_style,
                                                    Some(clip),
                                                );
                                                Self::draw_compare_wipe_line(
                                                    ui, image_rect, fraction,
                                                );
                                            }
                                        }
                                    }
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
                        self.sync_slideshow_anchor_for_frame(ctx, fs_idx, &state);
                        self.draw_slideshow_progress_indicator(ui, full_rect, ctx);
                        if !state.is_video {
                            self.draw_compare_pin_indicator(ui, full_rect, ctx);
                        }
                        fs_overlay_ms = overlay_t0.elapsed().as_secs_f64() * 1000.0;

                        // ── 動画 (エラー / 「準備中」のみ) ──
                        // Playback HUD / seek bar / Perf graph 等は native presenter の
                        // egui overlay 側 (`src/video/native_presenter/overlay_draw.rs`)
                        // が描画する。ここでは egui main window 側に出すべき
                        // 軽量インジケータだけ担当する。
                        let hud_t0 = std::time::Instant::now();
                        if state.is_video {
                            self.draw_video_hud(ui, full_rect, fs_idx);
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
                                if (self.video_tile_mode_active || self.video_tile_reopen_pending)
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
                                        let screen = self.video_tile_layout_size(fs_idx, ctx);
                                        self.video_tile_state =
                                            self.build_video_tile_state_for(fs_idx, screen);
                                        if self.video_tile_state.is_some() {
                                            self.video_tile_mode_active = true;
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
                        } else if !is_spread_double && !compare_wipe_active {
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
                            let tile_active = self.video_tile_mode_active;
                            #[cfg(not(windows))]
                            let tile_active = false;
                            let mut tile_pressed = false;
                            // VST ボタン: 動画モード + VST3 機能有効のときだけ表示
                            let show_vst3_button =
                                cfg!(windows) && is_video_mode && self.settings.vst3_enabled;
                            let vst3_panel_open = self.show_vst3_manager;
                            let mut vst3_pressed = false;
                            let mut copy_capture_pressed = false;
                            let mut window_mode_pressed = false;
                            // ウィンドウ / 全画面 切り替えボタン: 静止画フルスクリーン
                            // (= 非動画、Windows) のときだけ出す。動画は native HUD 側に
                            // 専用トグルがある。
                            let show_window_toggle = cfg!(windows) && !is_video_mode;
                            let slideshow_was_playing = self.slideshow_playing;
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
                                &mut copy_capture_pressed,
                                show_window_toggle,
                                embedded,
                                &mut window_mode_pressed,
                                self.cursor_hidden,
                            );
                            if copy_capture_pressed {
                                self.copy_image_capture_to_clipboard(fs_idx);
                            }
                            // ウィンドウ / 全画面 切り替えボタンが押された。
                            #[cfg(windows)]
                            if window_mode_pressed {
                                self.toggle_still_window_mode();
                                // 描画先 (embedded ⇔ 専用 viewport) の切替は次フレームの
                                // render_fullscreen_viewport で起きる。静止画は
                                // handle_fs_repaint が自発 repaint しないので、ルート
                                // ビューポートの次フレームを明示要求する。さもないと
                                // モード切替が次の入力まで反映されない (Codex P2)。
                                ctx.request_repaint_of(egui::ViewportId::ROOT);
                            }
                            if !slideshow_was_playing && self.slideshow_playing {
                                self.schedule_next_slideshow_from_now();
                            }
                            // ▦ タイルボタンが押されたら toggle_video_tile_mode に dispatch
                            #[cfg(windows)]
                            if tile_pressed {
                                let screen = self.video_tile_layout_size(fs_idx, ctx);
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

                        // ── 中央の境界ヒント (最初/最後の項目です…) ──
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
                        if self.cursor_hidden {
                            ctx.send_viewport_cmd(egui::ViewportCommand::CursorVisible(true));
                        }
                        self.cursor_hidden = false;
                    }
                    let idle = self
                        .cursor_last_activity
                        .map(|t| t.elapsed().as_secs_f32())
                        .unwrap_or(0.0);
                    let threshold = crate::video::native_presenter::CURSOR_HIDE_IDLE_SECS;
                    if clean && (idle >= threshold || self.cursor_hidden) {
                        if !self.cursor_hidden {
                            ctx.send_viewport_cmd(egui::ViewportCommand::CursorVisible(false));
                        }
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
            };
            if embedded {
                // in-window 静止画: メインウィンドウの egui ctx に直接描画する。
                render_fs_body(main_ctx, true);
            } else {
                // 従来: 専用フルスクリーン viewport を出してそこに描画する。
                main_ctx.show_viewport_immediate(fs_id, fs_builder, |vp_ctx, _class| {
                    render_fs_body(vp_ctx, false);
                });
            }
        }
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

        if !embedded {
            // embedded のときは専用 viewport を作っていないので shown フラグは
            // 立てない (close 後の viewport 後始末も走らせない)。
            self.fs_viewport_shown = true;
        }

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
        if let Some((hwnd, _)) = self.pending_native_video_output_hwnds_for_fs(fs_idx) {
            return Some(hwnd);
        }
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

    /// in-window モードで静止画 (= 非動画) フルスクリーンを表示中かどうか。
    ///
    /// true のとき `render_fullscreen_viewport` は専用 viewport ではなく
    /// メインウィンドウの egui ctx に直接 CentralPanel を描く (embedded)。
    /// 動画は native presenter (独立 / 子 HWND) 経路なのでここでは除外する。
    /// これにより in-window 動画 ⇔ 静止画をホイールで往復しても画面モードが
    /// 全画面↔ウィンドウで切り替わらず、一貫して main ウィンドウ内に収まる。
    ///
    /// 判定はフルスクリーンで表示しうる静止画系アイテム
    /// (Image / ZipImage / PdfPage / ZipSeparator) の **明示的な許可リスト**で行う。
    /// 「非 Video なら何でも embedded」にすると、`fullscreen_idx` が範囲外や
    /// コンテナ (Folder/ZipFile/PdfFile) を指す異常時に embedded 扱いになり、
    /// `update()` がグリッド描画を抑止して黒画面に閉じ込められる恐れがあるため。
    #[cfg(windows)]
    pub(crate) fn fullscreen_embedded_still_active(&self) -> bool {
        self.native_video_in_window_active
            && self.fullscreen_idx.is_some_and(|idx| {
                matches!(
                    self.items.get(idx),
                    Some(
                        GridItem::Image(_)
                            | GridItem::ZipImage { .. }
                            | GridItem::PdfPage { .. }
                            | GridItem::ZipSeparator { .. }
                    )
                )
            })
    }

    #[cfg(windows)]
    fn native_video_presenter_pending_for_fs(&self, fs_idx: usize) -> bool {
        if self.pending_native_video_output_active_for_fs(fs_idx) {
            let Some((hwnd, _)) = self.pending_native_video_output_hwnds_for_fs(fs_idx) else {
                return true;
            };
            return self.native_video_front_synced_hwnd != hwnd;
        }
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

    /// 動画フルスクリーンの黒 backdrop viewport が生きていれば、DWM フェードを
    /// 抑止しつつ 1 フレームで隠して破棄させる。`fs_viewport_shown` が false の
    /// ときは **何もしない** — 死んでいる viewport を `show_viewport_immediate` で
    /// 復活させてしまうと、`need_show=false` 経路で可視のまま再生成され、白い
    /// クラス背景がフラッシュするため (Plan B トグルで実害、2026-05-22)。
    /// `keep_fullscreen_viewport_alive` の cleanup と同じ手順。
    #[cfg(windows)]
    fn hide_native_video_black_backdrop_if_shown(&mut self, ctx: &egui::Context) {
        if !self.fs_viewport_shown {
            return;
        }
        let fs_id = self.fullscreen_viewport_id();
        let fs_builder = self.build_fullscreen_viewport_builder().with_visible(false);
        ctx.show_viewport_immediate(fs_id, fs_builder, |_ctx, _class| {});
        crate::dwm_transitions::disable_transitions_for_thread_windows();
        ctx.send_viewport_cmd_to(fs_id, egui::ViewportCommand::Visible(false));
        self.fs_viewport_shown = false;
    }

    #[cfg(windows)]
    fn show_native_video_black_backdrop(&mut self, ctx: &egui::Context, fs_idx: usize) {
        let fs_id = self.fullscreen_viewport_id();
        let need_show = !self.fs_viewport_shown;
        let fs_builder = self.build_fullscreen_viewport_builder_with_transparency(false);
        let fs_builder = if need_show {
            // Create the fullscreen backdrop HWND hidden first. The DWM
            // transition flag can only be applied after the HWND exists, so a
            // visible initial create can animate before the attribute lands.
            fs_builder.with_visible(false)
        } else {
            fs_builder
        };
        let expected_physical_rect = self.fullscreen_backdrop_physical_rect();
        let mut close_fs = false;
        ctx.show_viewport_immediate(fs_id, fs_builder, |ctx, _class| {
            // Visible な fullscreen viewport なので、native 動画の黒 backdrop 中も
            // IME 状態だけは通常 viewport と同じ入口で更新する。
            self.update_ime_state(ctx);
            if need_show {
                crate::dwm_transitions::disable_transitions_for_thread_windows();
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
            // Codex 2周目 P1: ノーマライズスキャン中なら handle_native_video_key_event の
            // ESC は cancel ルートに乗る。直後の `consume_key` で ESC を取るとフォールバック側で
            // close_fs = true になり、cancel + close が同フレームで走ってしまうので、
            // この呼び出し前後でスキャン状態の変化 (= ESC で cancel された) を検出して
            // close 判定をスキップする。
            let normalize_active_before = self.normalize_state.is_some();
            if !self.ime_input_active() {
                for key in native_video_key_events_from_ctx(ctx) {
                    self.handle_native_video_key_event(ctx, fs_idx, key);
                }
            }
            let normalize_cancelled_this_frame =
                normalize_active_before && self.normalize_state.is_none();
            let close_requested = ctx.input(|i| i.viewport().close_requested());
            let escape_pressed = !self.ime_input_active()
                && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
            if (close_requested || escape_pressed) && !normalize_cancelled_this_frame {
                close_fs = true;
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(egui::Color32::BLACK))
                .show(ctx, |_ui| {});
        });
        if self.native_video_presenter_hwnd_for_fs(fs_idx).is_none()
            && let (Some(main_hwnd), Some(expected)) = (self.main_hwnd, expected_physical_rect)
        {
            let raised = crate::dwm_transitions::raise_visible_thread_window_matching_rect(
                windows::Win32::Foundation::HWND(main_hwnd as *mut _),
                expected,
            );
            if need_show {
                crate::logger::log(format!(
                    "[native-video] raised fullscreen backdrop hwnd=0x{:x} main=0x{:x}",
                    raised.map(|hwnd| hwnd.0 as usize).unwrap_or(0),
                    main_hwnd as usize
                ));
            }
        }
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

    #[cfg(windows)]
    fn fullscreen_backdrop_physical_rect(&self) -> Option<windows::Win32::Foundation::RECT> {
        let center = self.last_outer_rect.map(|r| r.center())?;
        let ppp = self.last_pixels_per_point;
        let rect = crate::monitor::get_monitor_logical_rect_at(center.x * ppp, center.y * ppp)?;
        Some(windows::Win32::Foundation::RECT {
            left: (rect.min.x * ppp).round() as i32,
            top: (rect.min.y * ppp).round() as i32,
            right: (rect.max.x * ppp).round() as i32,
            bottom: (rect.max.y * ppp).round() as i32,
        })
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
        // in-window 静止画 (embedded) では本体 (render_fullscreen_viewport) の
        // handle_fs_key_input が同じ main ctx 上で直接キーを処理する。ここでも
        // 処理するとナビが二重発火するので委譲する。true を返して
        // handle_keyboard / handle_clipboard_shortcuts (= グリッド用) を抑止する。
        #[cfg(windows)]
        if self.fullscreen_embedded_still_active() {
            return true;
        }
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
        // マウスドライバ / AHK 経由で積まれた進む/戻る pending は **early-return より前に
        // drain** する。フォーカス無し / モーダル表示中で早期 return すると pending が
        // 次フレームに持ち越されて誤発火するため (Codex P2)。ブロック中は count を捨てる。
        let (browser_back_count, browser_forward_count) = crate::take_pending_mouse_nav();

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
        // マウス戻る/進む (Extra1/Extra2 = native XButton) を Ctrl+↑/↓ と等価に扱う。
        let mouse_back = ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Extra1));
        let mouse_forward = ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Extra2));
        // WM_APPCOMMAND / VK_BROWSER_BACK/FORWARD 経路 (上で関数頭で drain 済み) を消費。
        // 詳細は main.rs の `install_mouse_nav_hook` 参照。
        let browser_back = browser_back_count > 0;
        let browser_forward = browser_forward_count > 0;
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
        let key_ctrl_s_capture = !is_video_fs
            && self.fs_context_menu_idx.is_none()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::S));
        let key_compare_x = !is_video_fs
            && self.fs_context_menu_idx.is_none()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::X));
        let key_compare_alt_c = !is_video_fs
            && self.fs_context_menu_idx.is_none()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::ALT, egui::Key::C));
        let key_compare_shift_c = !is_video_fs
            && self.fs_context_menu_idx.is_none()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::C));
        let key_compare_c = !is_video_fs
            && self.fs_context_menu_idx.is_none()
            && !key_compare_alt_c
            && !key_compare_shift_c
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::C));
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
        // P: 現在表示中アイテムを親コンテナの代表サムネに固定 / 解除。
        // 動画フルスクリーンの P は handle_video_input が先に「現在フレームをピン留め」として
        // consume するため、ここでは静止画系アイテムだけを対象にする。
        let current_item_is_video = matches!(self.items.get(fs_idx), Some(GridItem::Video(_)));
        let key_p_pin = !current_item_is_video
            && ctx.input_mut(|i| {
                !i.modifiers.shift
                    && !i.modifiers.alt
                    && !i.modifiers.ctrl
                    && i.consume_key(egui::Modifiers::NONE, egui::Key::P)
            });

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
        if key_p_pin {
            self.toggle_folder_pin_for_idx(fs_idx);
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
        // T / Shift+T / Alt+T: ポストフィルタ (レトロ系) サイクル (次 / 前 / なしリセット)
        // P はグリッド / 動画フルスクリーンのピン留めに統一する。F は動画の FPS/Perf 表示に
        // 使っているため、ポストフィルタは T (Tone / posT filter) に割り当てる。
        // 同様に Alt+T → Shift+T → T の順で consume (matches_logically 対策)。
        let key_t_alt = ctx.input_mut(|i| i.consume_key(egui::Modifiers::ALT, egui::Key::T));
        let key_t_shift = ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::T));
        let key_t = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::T));

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

        // T / Shift+T / Alt+T: ポストフィルタの次/前/なしへ切替。
        // AI 再実行は発生させないため色調キャッシュのみクリア。
        if key_t || key_t_shift || key_t_alt {
            let scope = self.resolve_adjust_scope(fs_idx);
            let mut params = self.effective_params(fs_idx).clone();
            let all = crate::adjustment::PostFilter::ALL;
            let cur = all
                .iter()
                .position(|f| *f == params.post_filter)
                .unwrap_or(0);
            let next_idx = if key_t_alt {
                0
            } else if key_t_shift {
                (cur + all.len() - 1) % all.len()
            } else {
                (cur + 1) % all.len()
            };
            let next = all[next_idx];
            params.post_filter = next;
            self.show_feedback_toast(format!("[T: {} / {}]", scope.label(), next.display_label()));
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

        if key_compare_x {
            self.toggle_compare_pin_from_current(ctx, fs_idx);
        }
        if key_compare_c {
            self.toggle_compare_pinned_view(ctx, fs_idx);
        }
        if key_compare_shift_c {
            self.toggle_compare_wipe_mode(ctx, fs_idx);
        }
        if key_compare_alt_c {
            self.toggle_compare_diff_mode(ctx, fs_idx);
        }

        if esc && self.compare_view_mode.is_overlay() {
            self.compare_view_mode = crate::app::CompareViewMode::Off;
            self.compare_wipe_dragging = false;
            self.show_feedback_toast("[比較: Normal]".to_string());
        } else if esc {
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

        if key_ctrl_s_capture {
            self.save_image_capture_to_file(ctx, fs_idx);
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
                self.schedule_next_slideshow_from_now();
            }
        }

        // Space: スライドショー中→停止、停止中→画像をチェック
        if key_space {
            if self.slideshow_playing {
                self.slideshow_playing = false;
            } else {
                let mut checked_now = None;
                match self.items.get(fs_idx) {
                    Some(GridItem::Image(_))
                    | Some(GridItem::Video(_))
                    | Some(GridItem::ZipImage { .. })
                    | Some(GridItem::PdfPage { .. }) => {
                        let checked = if self.checked.contains(&fs_idx) {
                            self.checked.remove(&fs_idx);
                            false
                        } else {
                            self.checked.insert(fs_idx);
                            true
                        };
                        checked_now = Some(checked);
                    }
                    _ => {}
                }
                #[cfg(windows)]
                if let Some(checked) = checked_now
                    && matches!(self.items.get(fs_idx), Some(GridItem::Video(_)))
                    && let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx)
                {
                    player.set_native_checked(checked);
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
        if ctrl_d || mouse_forward || browser_forward {
            action.ctrl_nav = Some(1);
        }
        if ctrl_u || mouse_back || browser_back {
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

    fn handle_compare_wipe_drag(&mut self, ctx: &egui::Context, image_rect: egui::Rect) -> bool {
        let crate::app::CompareViewMode::Wipe { fraction } = self.compare_view_mode else {
            self.compare_wipe_dragging = false;
            return false;
        };
        if image_rect.width() <= 1.0 {
            return false;
        }

        let (primary_pressed, primary_down, primary_released, pointer_pos) = ctx.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.primary_released(),
                i.pointer.interact_pos().or_else(|| i.pointer.hover_pos()),
            )
        });
        if primary_released {
            let was_dragging = self.compare_wipe_dragging;
            self.compare_wipe_dragging = false;
            return was_dragging;
        }

        let line_x = image_rect.left() + image_rect.width() * fraction.clamp(0.05, 0.95);
        let hit = pointer_pos
            .map(|p| image_rect.contains(p) && (p.x - line_x).abs() <= 14.0)
            .unwrap_or(false);
        if hit {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if primary_pressed && hit {
            self.compare_wipe_dragging = true;
        }
        if self.compare_wipe_dragging && primary_down {
            if let Some(pos) = pointer_pos {
                let new_fraction =
                    ((pos.x - image_rect.left()) / image_rect.width()).clamp(0.05, 0.95);
                self.compare_view_mode = crate::app::CompareViewMode::Wipe {
                    fraction: new_fraction,
                };
                ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                ctx.request_repaint();
            }
            return true;
        }
        false
    }

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

        let compare_wipe_active = matches!(
            self.compare_view_mode,
            crate::app::CompareViewMode::Wipe { .. }
        );

        // ── ホイール ──
        // パネル領域内ではホイールナビゲーションを抑制
        let panel_w = METADATA_PANEL_WIDTH.min(full_rect.width() * 0.5);
        let panel_left = full_rect.max.x - panel_w;
        let hover_threshold = full_rect.max.x - full_rect.width() * 0.25;
        let has_right_panel = self.show_metadata_panel;
        let left_panel_w =
            crate::ui_adjustment_panel::LEFT_PANEL_WIDTH.min(full_rect.width() * 0.3);
        // When the OS cursor is hidden, egui still exposes the last hover position.
        // Treat that position as stale and block passive hover side effects until a
        // real input event revives the cursor.
        let passive_hover_enabled = !self.cursor_hidden;
        let cursor_in_panel = passive_hover_enabled
            && ctx.input(|i| {
                i.pointer
                    .hover_pos()
                    .map(|p| {
                        let in_right = !compare_wipe_active
                            && p.x > panel_left
                            && p.y >= 60.0
                            && (has_right_panel || p.x > hover_threshold);
                        let in_left = !compare_wipe_active
                            && self.adjustment_mode
                            && p.x < full_rect.min.x + left_panel_w
                            && p.y >= 60.0;
                        in_right || in_left
                    })
                    .unwrap_or(false)
            });

        // 左端・上端・右端のホバーでオーバーレイ（上バー＋左パネル＋右パネル）を同時表示/非表示
        // 消しゴムモード中は自前のパネルを左端に描いているためエッジ発火を抑制する。
        // 加えて、消しゴムモードに入る前から adjustment_mode が立っていると、消しゴムパネルが
        // 左端を占有している間 edge_hover が常に true 扱いになり off へ遷移できないので、
        // 強制的に落とす。
        if compare_wipe_active {
            if self.adjustment_mode && !self.adjustment_dragging {
                self.adjustment_mode = false;
            }
        } else if self.erase_mode {
            self.adjustment_mode = false;
        } else {
            let edge_hover = passive_hover_enabled
                && ctx.input(|i| {
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

        let compare_drag_rect = if self.analysis_mode && !is_spread_double {
            analysis_image_rect(full_rect)
        } else {
            full_rect
        };
        if !cursor_in_panel && self.handle_compare_wipe_drag(ctx, compare_drag_rect) {
            return (0, false);
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
        let in_video_tile = self.video_tile_mode_active;
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
                        // Phase 7.I 追加: 動画オープン直後の中央 2 ボタン
                        // (最初から / 続きから) の領域を除外。
                        let tile_active = self.video_tile_mode_active;
                        let pos_opt = fs_response.interact_pointer_pos();
                        // 旧 egui HUD は撤去済 (native presenter overlay が代替)。
                        // egui main window 側で intercept すべき HUD 矩形は無いので常に false。
                        let in_hud = false;
                        let in_video_panel = pos_opt
                            .map(|p| {
                                let left_thresh = full_rect.min.x + full_rect.width() * 0.25;
                                let right_thresh = full_rect.max.x - full_rect.width() * 0.25;
                                (p.x < left_thresh || p.x > right_thresh)
                                    && p.y >= full_rect.min.y + 44.0
                            })
                            .unwrap_or(false);
                        // 中央 2 ボタン (オープン直後の初回 pause prompt) の領域だけを除外。
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
                                    && p.initial_pause_controls_pending()
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
            // 旧 egui HUD は撤去済 (native presenter overlay が右クリックも独自処理)。
            // egui main window 側に右クリックを吸収すべき HUD 矩形は無い。

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

    fn slideshow_interval_duration(&self) -> std::time::Duration {
        let secs = self.settings.slideshow_interval_secs;
        let secs = if secs.is_finite() {
            secs.clamp(0.5, 30.0)
        } else {
            3.0
        };
        std::time::Duration::from_secs_f32(secs)
    }

    fn schedule_next_slideshow_from_now(&mut self) {
        self.slideshow_next_at = std::time::Instant::now() + self.slideshow_interval_duration();
        self.slideshow_anchor_idx = self.fullscreen_idx;
    }

    fn current_slideshow_frame_ready(&self, fs_idx: usize, state: &FsFrameState) -> bool {
        if state.separator_text.is_some() {
            return true;
        }
        let has_own_thumb = matches!(
            self.thumbnails.get(fs_idx),
            Some(ThumbnailState::Loaded { .. })
        );
        state.tex.is_some() || has_own_thumb || state.fs_load_failed
    }

    fn sync_slideshow_anchor_for_frame(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        state: &FsFrameState,
    ) {
        if !self.slideshow_playing {
            return;
        }
        if state.is_video {
            self.slideshow_playing = false;
            self.slideshow_anchor_idx = None;
            return;
        }
        let ready = self.current_slideshow_frame_ready(fs_idx, state);
        if self.slideshow_anchor_idx == Some(fs_idx) {
            if !ready {
                self.slideshow_anchor_idx = None;
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
            return;
        }
        if ready {
            self.schedule_next_slideshow_from_now();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    pub(crate) fn open_fullscreen_from_fs_navigation(&mut self, ctx: &egui::Context, idx: usize) {
        #[cfg(windows)]
        if self.try_start_video_tile_fast_swap(ctx, idx) {
            return;
        }
        #[cfg(windows)]
        if self.try_start_native_video_fast_swap(ctx, idx, None, false) {
            return;
        }

        #[cfg(windows)]
        let restore_video_tile = self.video_tile_mode_active;

        #[cfg(windows)]
        if restore_video_tile {
            self.video_tile_state = None;
            self.video_tile_swap_pending = None;
        } else {
            self.cancel_stale_video_tile_reopen(self.fullscreen_idx, "fs-navigation");
        }

        self.open_fullscreen(idx);

        #[cfg(windows)]
        {
            if restore_video_tile && matches!(self.items.get(idx), Some(GridItem::Video(_))) {
                self.video_tile_mode_active = true;
                self.video_tile_reopen_pending = true;
                self.video_tile_reopen_deadline =
                    Some(std::time::Instant::now() + std::time::Duration::from_secs(3));
                ctx.request_repaint();
            } else if restore_video_tile {
                self.video_tile_mode_active = false;
                self.video_tile_reopen_pending = false;
                self.video_tile_reopen_deadline = None;
            }
        }
    }

    fn open_fullscreen_from_slideshow_navigation(&mut self, ctx: &egui::Context, idx: usize) {
        let cursor_last_activity = self.cursor_last_activity;
        let cursor_hidden = self.cursor_hidden;
        self.open_fullscreen_from_fs_navigation(ctx, idx);
        // `open_fullscreen` resets cursor idleness for a new fullscreen entry. Slideshow
        // advances are timer-driven fullscreen-internal navigation, so keep the idle
        // countdown/hidden state continuous across image changes; otherwise every slide
        // briefly revives the OS cursor.
        self.cursor_last_activity = cursor_last_activity;
        self.cursor_hidden = cursor_hidden;
        if cursor_hidden {
            // The per-frame hide loop is skipped while panels/HUD are visible; assert the
            // OS cursor state here so timer-driven slides cannot flash it back on.
            // This helper runs after `show_viewport_immediate`, so target the fullscreen
            // viewport explicitly rather than the root viewport context.
            ctx.send_viewport_cmd_to(
                self.fullscreen_viewport_id(),
                egui::ViewportCommand::CursorVisible(false),
            );
            ctx.set_cursor_icon(egui::CursorIcon::None);
        }
    }

    pub(crate) fn handle_fullscreen_ctrl_nav_context(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        forward: bool,
        native_toast: bool,
    ) {
        if self.fs_nav_is_locked() {
            return;
        }

        if self.global_search.active {
            if matches!(
                self.global_search.view,
                crate::global_search_ui::GlobalSearchView::DrilledInto { .. }
            ) {
                self.global_search_ctrl_nav_fullscreen(ctx, forward);
            } else {
                self.show_fullscreen_nav_noop(ctx, FsNavNoOpReason::SearchResultList, native_toast);
            }
            return;
        }

        if self.favsearch.active {
            let Some(root) = self.favsearch.nav_stack.first().cloned() else {
                self.show_fullscreen_nav_noop(ctx, FsNavNoOpReason::SearchResultList, native_toast);
                return;
            };
            let Some(current) = self.favsearch.nav_stack.last().cloned() else {
                return;
            };
            self.capture_fs_nav_holdover(fs_idx);
            self.start_folder_nav(
                current,
                forward,
                crate::app::FolderNavMode::Favsearch {
                    root,
                    fullscreen: true,
                },
            );
            return;
        }

        if self.show_search_bar {
            self.cancel_pending_folder_nav();
            self.show_fullscreen_nav_noop(ctx, FsNavNoOpReason::LocalFilterActive, native_toast);
            return;
        }

        if let Some(cur) = self.current_folder.clone() {
            self.capture_fs_nav_holdover(fs_idx);
            self.start_folder_nav(cur, forward, crate::app::FolderNavMode::Fullscreen);
        }
    }

    fn show_fullscreen_nav_noop(
        &mut self,
        ctx: &egui::Context,
        reason: FsNavNoOpReason,
        native_toast: bool,
    ) {
        #[cfg(windows)]
        if native_toast {
            self.show_native_video_overlay_toast(Self::nav_noop_title(reason).to_string(), true);
            self.mark_native_video_hud_activity(ctx);
            return;
        }

        #[cfg(not(windows))]
        let _ = (ctx, native_toast);

        self.fs_boundary_hint = Some(FsBoundaryHint::NavNoOp {
            reason,
            at: std::time::Instant::now(),
        });
    }

    pub(crate) fn nav_noop_title(reason: FsNavNoOpReason) -> &'static str {
        match reason {
            FsNavNoOpReason::LocalFilterActive => "Ctrl+F検索中はフォルダ移動しません",
            FsNavNoOpReason::SearchResultList => "検索結果を開いてからCtrl+↑↓で移動できます",
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
            // keep_fullscreen_viewport_alive の cleanup フレーム (Visible(false) 送信) を保証。
            // 修正後の keep_alive はアイドル時ゼロコスト早期 return するため、偶発的な
            // input/focus repaint に頼らず明示的に次フレームを起こす。
            ctx.request_repaint();
        }
        // fast-swap (動画タイル / native 動画) が進行中なら、swap 機構側が
        // 表示遷移を完結させるので、handle_fs_navigation 経由の通常 nav 経路は
        // 二重発火を避けるため早期 return する。
        #[cfg(windows)]
        if !close_fs
            && (self.video_tile_swap_pending.is_some()
                || self.native_video_fast_swap_pending.is_some())
        {
            return;
        }
        // Ctrl+↑↓ はフルスクリーンを保ったまま現在コンテキストの前後へ飛び、
        // 移動先の先頭 image-like を開く。self.selected も合わせて更新するので、
        // ここからフルスクリーンを閉じたときグリッド側のカーソルが最後に観た項目に残る。
        //
        // 実装上: `navigate_folder_with_skip` は DFS + `read_dir` で UI スレッドを
        // ブロックし得るので (深い階層だと 100ms 級)、ここでは発火だけ行い、
        // 実際の close_fullscreen / load_folder / open_fullscreen は
        // `apply_folder_nav_result` (FolderNavMode::Fullscreen ブランチ) に任せる。
        if let Some(delta) = ctrl_nav {
            self.handle_fullscreen_ctrl_nav_context(ctx, fs_idx, delta > 0, false);
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
            let anchored = self
                .fullscreen_idx
                .is_some_and(|idx| self.slideshow_anchor_idx == Some(idx));
            if !anchored {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            } else {
                if now >= self.slideshow_next_at {
                    let mut advanced = false;
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
                                self.slideshow_anchor_idx = None;
                                self.open_fullscreen_from_slideshow_navigation(ctx, idx);
                                self.selected = Some(idx);
                                self.scroll_to_selected = true;
                                advanced = true;
                            }
                            None => {
                                self.slideshow_playing = false;
                                self.slideshow_anchor_idx = None;
                            }
                        }
                    }
                    if advanced {
                        ctx.request_repaint();
                    }
                }
                if self.slideshow_playing {
                    let remaining = self.slideshow_next_at.saturating_duration_since(now);
                    ctx.request_repaint_after(remaining.min(std::time::Duration::from_millis(100)));
                }
            }
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

    fn ensure_compare_pinned_texture(
        &mut self,
        ctx: &egui::Context,
    ) -> Option<egui::TextureHandle> {
        let slot = self.pinned_compare_slot.as_mut()?;
        if slot.texture.is_none() {
            let tex = ctx.load_texture(
                format!(
                    "compare_pin_{}_{}x{}",
                    slot.source_idx, slot.source_size[0], slot.source_size[1]
                ),
                slot.pixels.as_ref().clone(),
                egui::TextureOptions::LINEAR,
            );
            slot.texture = Some(tex);
        }
        slot.texture.clone()
    }

    fn compare_prepared_pair_matches(&self, fs_idx: usize) -> bool {
        let Some(slot) = self.pinned_compare_slot.as_ref() else {
            return false;
        };
        self.compare_prepared_pair.as_ref().is_some_and(|pair| {
            pair.current_idx == fs_idx && pair.pinned_source_idx == slot.source_idx
        })
    }

    fn ensure_compare_prepared_pair(&mut self, ctx: &egui::Context, fs_idx: usize) -> bool {
        let Some(slot) = self.pinned_compare_slot.as_ref() else {
            return false;
        };
        if self.compare_prepared_pair_matches(fs_idx) {
            return true;
        }
        if self
            .compare_prepare_pending
            .as_ref()
            .is_some_and(|pending| {
                pending.current_idx == fs_idx && pending.pinned_source_idx == slot.source_idx
            })
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return false;
        }

        let pinned_source_idx = slot.source_idx;
        let pinned_size = slot.source_size;
        let pinned_pixels = Arc::clone(&slot.pixels);
        let current_work = match self.prepare_capture_pixel_work(fs_idx) {
            Ok(work) => work,
            Err(err) => {
                self.compare_view_mode = crate::app::CompareViewMode::Off;
                self.compare_wipe_dragging = false;
                self.show_feedback_toast(err);
                return false;
            }
        };
        self.compare_prepared_pair = None;
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("compare-prepare".into())
            .spawn(move || {
                let result = crate::capture::run_pixel_work(current_work).and_then(
                    |(_basename, width, height, current_rgba)| {
                        let pinned_source_rgba =
                            crate::capture::color_image_to_rgba(pinned_pixels.as_ref());
                        let pinned_rgba = crate::capture::align_rgba_to_canvas_lanczos(
                            pinned_size[0] as u32,
                            pinned_size[1] as u32,
                            &pinned_source_rgba,
                            width,
                            height,
                        )?;
                        let diff_rgba = crate::capture::diff_rgba_color(
                            width,
                            height,
                            &pinned_rgba,
                            &current_rgba,
                        )?;
                        Ok(crate::app::ComparePrepareResult {
                            current_idx: fs_idx,
                            pinned_source_idx,
                            width,
                            height,
                            pinned_rgba,
                            current_rgba,
                            diff_rgba,
                        })
                    },
                );
                let _ = tx.send(result);
            });

        match thread {
            Ok(_) => {
                self.compare_prepare_pending = Some(crate::app::ComparePreparePending {
                    current_idx: fs_idx,
                    pinned_source_idx,
                    rx,
                });
                self.show_feedback_toast("比較表示を準備中".to_string());
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            Err(err) => {
                self.show_feedback_toast(format!("比較 worker を開始できません: {err}"));
            }
        }
        false
    }

    fn ensure_compare_prepared_texture(
        &mut self,
        ctx: &egui::Context,
        kind: ComparePreparedTextureKind,
    ) -> Option<egui::TextureHandle> {
        let pair = self.compare_prepared_pair.as_mut()?;
        let key = pair.key;
        let target_size = pair.target_size;
        let (slot, pixels, label) = match kind {
            ComparePreparedTextureKind::Pinned => (
                &mut pair.pinned_texture,
                Arc::clone(&pair.pinned_pixels),
                "pinned",
            ),
            ComparePreparedTextureKind::Current => (
                &mut pair.current_texture,
                Arc::clone(&pair.current_pixels),
                "current",
            ),
            ComparePreparedTextureKind::Diff => (
                &mut pair.diff_texture,
                Arc::clone(&pair.diff_pixels),
                "diff",
            ),
        };
        if slot.is_none() {
            let tex = ctx.load_texture(
                format!(
                    "compare_prepared_{}_{}_{}x{}",
                    label, key, target_size[0], target_size[1]
                ),
                pixels.as_ref().clone(),
                egui::TextureOptions::LINEAR,
            );
            *slot = Some(tex);
        }
        slot.clone()
    }

    fn compare_image_draw_rect(
        full_rect: egui::Rect,
        image_size: [usize; 2],
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) -> Option<egui::Rect> {
        let tex_size = egui::vec2(image_size[0] as f32, image_size[1] as f32);
        if tex_size.x <= 0.0
            || tex_size.y <= 0.0
            || full_rect.width() <= 0.0
            || full_rect.height() <= 0.0
        {
            return None;
        }
        let fit_scale = (full_rect.width() / tex_size.x).min(full_rect.height() / tex_size.y);
        let (total_scale, center) = match zoom_pan {
            Some((zoom, pan)) => (fit_scale * zoom, full_rect.center() + pan),
            None => (fit_scale, full_rect.center()),
        };
        Some(egui::Rect::from_center_size(center, tex_size * total_scale))
    }

    #[cfg(windows)]
    fn compare_shader_shape(
        &self,
        image_rect: egui::Rect,
        pair: &crate::app::ComparePreparedPair,
        mode: crate::compare_wgpu::CompareShaderMode,
        wipe_fraction: f32,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) -> Option<(egui::Rect, egui::Shape)> {
        let target_format = self.wgpu_render_state.as_ref()?.target_format;
        let draw_rect = Self::compare_image_draw_rect(image_rect, pair.target_size, zoom_pan)?;
        let callback = crate::compare_wgpu::CompareShaderCallback {
            key: pair.key,
            width: pair.target_size[0] as u32,
            height: pair.target_size[1] as u32,
            pinned_rgba: Arc::clone(&pair.pinned_rgba),
            current_rgba: Arc::clone(&pair.current_rgba),
            mode,
            wipe_fraction,
            target_format,
        };
        Some((
            draw_rect,
            egui::Shape::Callback(egui_wgpu::Callback::new_paint_callback(draw_rect, callback)),
        ))
    }

    #[cfg(not(windows))]
    fn compare_shader_shape(
        &self,
        _image_rect: egui::Rect,
        _pair: &crate::app::ComparePreparedPair,
        _mode: crate::compare_wgpu::CompareShaderMode,
        _wipe_fraction: f32,
        _zoom_pan: Option<(f32, egui::Vec2)>,
    ) -> Option<(egui::Rect, egui::Shape)> {
        None
    }

    fn draw_compare_prepared_mode(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        image_rect: egui::Rect,
        mode: crate::app::CompareViewMode,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) -> bool {
        if self.compare_prepared_pair.is_none() {
            return false;
        }

        let shader_shape = self
            .compare_prepared_pair
            .as_ref()
            .and_then(|pair| match mode {
                crate::app::CompareViewMode::Wipe { fraction } => self.compare_shader_shape(
                    image_rect,
                    pair,
                    crate::compare_wgpu::CompareShaderMode::Wipe,
                    fraction,
                    zoom_pan,
                ),
                crate::app::CompareViewMode::Diff => self.compare_shader_shape(
                    image_rect,
                    pair,
                    crate::compare_wgpu::CompareShaderMode::Diff,
                    0.5,
                    zoom_pan,
                ),
                _ => None,
            });
        if let Some((draw_rect, shape)) = shader_shape {
            let bg_style = self.fs_bg_style(ctx);
            paint_transparent_bg(ui.painter(), draw_rect, &bg_style);
            ui.painter().add(shape);
            if let crate::app::CompareViewMode::Wipe { fraction } = mode {
                Self::draw_compare_wipe_line(ui, image_rect, fraction);
            }
            return true;
        }

        match mode {
            crate::app::CompareViewMode::PinnedNormal => {
                let tex =
                    self.ensure_compare_prepared_texture(ctx, ComparePreparedTextureKind::Pinned);
                let Some(tex) = tex.as_ref() else {
                    return false;
                };
                let bg_style = self.fs_bg_style(ctx);
                Self::draw_compare_pinned_image(ui, image_rect, tex, zoom_pan, &bg_style, None);
                true
            }
            crate::app::CompareViewMode::Wipe { fraction } => {
                let current =
                    self.ensure_compare_prepared_texture(ctx, ComparePreparedTextureKind::Current);
                let pinned =
                    self.ensure_compare_prepared_texture(ctx, ComparePreparedTextureKind::Pinned);
                let (Some(current), Some(pinned)) = (current.as_ref(), pinned.as_ref()) else {
                    return false;
                };
                let bg_style = self.fs_bg_style(ctx);
                Self::draw_compare_pinned_image(ui, image_rect, current, zoom_pan, &bg_style, None);
                let wipe_x = image_rect.left() + image_rect.width() * fraction.clamp(0.05, 0.95);
                let clip =
                    egui::Rect::from_min_max(image_rect.min, egui::pos2(wipe_x, image_rect.max.y));
                Self::draw_compare_pinned_image(
                    ui,
                    image_rect,
                    pinned,
                    zoom_pan,
                    &bg_style,
                    Some(clip),
                );
                Self::draw_compare_wipe_line(ui, image_rect, fraction);
                true
            }
            crate::app::CompareViewMode::Diff => {
                let tex =
                    self.ensure_compare_prepared_texture(ctx, ComparePreparedTextureKind::Diff);
                let Some(tex) = tex.as_ref() else {
                    return false;
                };
                let bg_style = self.fs_bg_style(ctx);
                Self::draw_compare_pinned_image(ui, image_rect, tex, zoom_pan, &bg_style, None);
                true
            }
            crate::app::CompareViewMode::Off => false,
        }
    }

    fn draw_compare_pinned_image(
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
        tex: &egui::TextureHandle,
        zoom_pan: Option<(f32, egui::Vec2)>,
        bg_style: &FsBgStyle<'_>,
        clip_rect: Option<egui::Rect>,
    ) {
        let Some(img_rect) =
            Self::compare_image_draw_rect(full_rect, [tex.size()[0], tex.size()[1]], zoom_pan)
        else {
            return;
        };
        let clip = clip_rect
            .map(|r| r.intersect(full_rect))
            .unwrap_or(full_rect);
        let painter = ui.painter().with_clip_rect(clip);
        paint_transparent_bg(&painter, img_rect, bg_style);
        painter.image(
            tex.id(),
            img_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }

    fn draw_compare_wipe_line(ui: &mut egui::Ui, image_rect: egui::Rect, fraction: f32) {
        let x = image_rect.left() + image_rect.width() * fraction.clamp(0.05, 0.95);
        ui.painter().line_segment(
            [
                egui::pos2(x, image_rect.top()),
                egui::pos2(x, image_rect.bottom()),
            ],
            egui::Stroke::new(2.0, egui::Color32::from_white_alpha(150)),
        );
    }

    fn draw_compare_pin_indicator(
        &mut self,
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
        ctx: &egui::Context,
    ) {
        let Some(display_name) = self
            .pinned_compare_slot
            .as_ref()
            .map(|slot| slot.display_name.clone())
        else {
            return;
        };
        let tex = self.ensure_compare_pinned_texture(ctx);

        let max_label_chars = 28usize;
        let label = if display_name.chars().count() > max_label_chars {
            let mut s: String = display_name.chars().take(max_label_chars).collect();
            s.push('…');
            format!("比較中: {s}")
        } else {
            format!("比較中: {display_name}")
        };
        let font = egui::FontId::proportional(13.0);
        let galley = ui
            .painter()
            .layout_no_wrap(label, font, egui::Color32::WHITE);
        let thumb_size = egui::vec2(72.0, 54.0);
        let width = (thumb_size.x + 10.0 + galley.size().x).min(full_rect.width() - 32.0);
        let panel_size = egui::vec2(width + 16.0, thumb_size.y + 16.0);
        let panel_rect = egui::Rect::from_min_size(
            egui::pos2(
                full_rect.max.x - panel_size.x - 18.0,
                full_rect.max.y - panel_size.y - 18.0,
            ),
            panel_size,
        );
        ui.painter().rect_filled(
            panel_rect,
            4.0,
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 185),
        );
        let thumb_rect =
            egui::Rect::from_min_size(panel_rect.min + egui::vec2(8.0, 8.0), thumb_size);
        ui.painter().rect_filled(
            thumb_rect,
            2.0,
            egui::Color32::from_rgba_unmultiplied(35, 35, 35, 230),
        );
        if let Some(tex) = tex.as_ref() {
            let tex_size = tex.size_vec2();
            if tex_size.x > 0.0 && tex_size.y > 0.0 {
                let scale = (thumb_rect.width() / tex_size.x).min(thumb_rect.height() / tex_size.y);
                let draw_rect = egui::Rect::from_center_size(thumb_rect.center(), tex_size * scale);
                ui.painter().image(
                    tex.id(),
                    draw_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
        }
        let text_pos = egui::pos2(
            thumb_rect.max.x + 10.0,
            panel_rect.center().y - galley.size().y * 0.5,
        );
        ui.painter().galley(text_pos, galley, egui::Color32::WHITE);
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

    fn draw_slideshow_progress_indicator(
        &self,
        ui: &egui::Ui,
        full_rect: egui::Rect,
        ctx: &egui::Context,
    ) {
        if !self.slideshow_playing {
            return;
        }

        let now = std::time::Instant::now();
        let interval = self.slideshow_interval_duration();
        let interval_secs = interval.as_secs_f32().max(0.001);
        let anchored = self
            .fullscreen_idx
            .is_some_and(|idx| self.slideshow_anchor_idx == Some(idx));
        let progress = if anchored {
            let remaining = self.slideshow_next_at.saturating_duration_since(now);
            ((interval_secs - remaining.as_secs_f32()) / interval_secs).clamp(0.0, 1.0)
        } else {
            0.0
        };

        let painter = ui.painter();
        let center = egui::pos2(full_rect.max.x - 22.0, full_rect.min.y + 22.0);
        let radius = 8.0;
        painter.circle_stroke(
            center,
            radius,
            egui::Stroke::new(1.2, egui::Color32::from_white_alpha(42)),
        );

        if progress > 0.0 {
            let start = -std::f32::consts::FRAC_PI_2;
            let sweep = std::f32::consts::TAU * progress;
            let segments = ((36.0 * progress).ceil() as usize).clamp(2, 36);
            let mut points = Vec::with_capacity(segments + 1);
            for i in 0..=segments {
                let t = i as f32 / segments as f32;
                let angle = start + sweep * t;
                points.push(egui::pos2(
                    center.x + radius * angle.cos(),
                    center.y + radius * angle.sin(),
                ));
            }
            painter.add(egui::Shape::line(
                points,
                egui::Stroke::new(2.0, egui::Color32::from_white_alpha(120)),
            ));
        }

        let repaint = if anchored {
            self.slideshow_next_at
                .saturating_duration_since(now)
                .min(std::time::Duration::from_millis(100))
        } else {
            std::time::Duration::from_millis(100)
        };
        ctx.request_repaint_after(repaint);
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
    fn fs_ui_is_clean(&self, ctx: &egui::Context, full_rect: egui::Rect, _is_video: bool) -> bool {
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        // Once the cursor is hidden, the last hover position is stale until a real input
        // event arrives. Do not let that passive position keep hover UI "visible" and
        // immediately revive the OS cursor after slideshow advances.
        let passive_hover_enabled = !self.cursor_hidden;
        let in_top = passive_hover_enabled && pointer.is_some_and(|p| p.y < TOP_BAR_HOVER_Y);
        let in_right = passive_hover_enabled
            && pointer.is_some_and(|p| p.x > full_rect.max.x - full_rect.width() * 0.25);
        // 動画再生中の HUD / speed popup は native presenter overlay 側で管理されるため
        // egui main window のカーソル可視判定からは除外する (= 旧 egui HUD は撤去済)。
        !in_top
            && !in_right
            && !self.show_metadata_panel
            && !self.adjustment_mode
            && !self.erase_mode
            && !self.analysis_mode
            && !self.spread_popup_open
            && self.fs_context_menu_idx.is_none()
            && !self.any_dialog_open()
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
        copy_capture_pressed: &mut bool,
        // ウィンドウ / 全画面 切り替えボタン (× の左)。show=表示するか、
        // in_window=現在 in-window 表示中か、pressed=クリックされたか。
        show_window_toggle: bool,
        in_window_mode: bool,
        window_mode_pressed: &mut bool,
        cursor_hidden: bool,
    ) {
        let hover_in_top = ctx.input(|i| {
            !cursor_hidden
                && i.pointer
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
        let close_resp = close_resp.hover_tip_dark("閉じる [Esc]");
        if close_resp.clicked() {
            *close_fs = true;
        }
        if close_resp.hovered() {
            *nav_delta = 0;
        }
        next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;

        // ⊞ ウィンドウ / 全画面 切り替えボタン (× の左)。
        // native 動画 HUD のトグルボタンと同じ役割を、静止画フルスクリーンの
        // egui ホバーバーに置く。
        if show_window_toggle {
            let wm_resp = draw_bar_button(
                ui,
                next_x,
                bar_rect.min.y + BAR_BUTTON_MARGIN,
                "fs_window_mode_btn",
                |hovered| bar_button_bg(hovered, in_window_mode),
                in_window_mode,
                |p, c, r| draw_window_toggle_icon(p, c, r),
            );
            let wm_resp = wm_resp.hover_tip_dark(if in_window_mode {
                "全画面表示に切り替え"
            } else {
                "ウィンドウ内表示に切り替え"
            });
            if wm_resp.clicked() {
                *window_mode_pressed = true;
            }
            if wm_resp.hovered() {
                *nav_delta = 0;
            }
            next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;
        }

        // 📷 キャプチャコピー (画像のみ)。Ctrl+S のファイル保存と同じ snapshot 経路を使う。
        if !is_video {
            let camera_resp = draw_bar_button(
                ui,
                next_x,
                bar_rect.min.y + BAR_BUTTON_MARGIN,
                "fs_capture_copy_btn",
                |hovered| bar_button_bg(hovered, false),
                false,
                |p, c, r| draw_camera_icon(p, c, r),
            );
            let camera_resp = camera_resp
                .hover_tip_dark("クリック: クリップボードにコピー\nCtrl+S: ファイル保存");
            if camera_resp.clicked() {
                *copy_capture_pressed = true;
            }
            if camera_resp.hovered() {
                *nav_delta = 0;
            }
            next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;
        }

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
            let vst_resp = vst_resp.hover_tip_dark(if vst3_panel_open {
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
            let tile_resp = tile_resp.hover_tip_dark(if tile_active {
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
                play_resp.hover_tip_dark("スライドショー停止")
            } else {
                play_resp.hover_tip_dark("スライドショー")
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
            let rcw_resp = rcw_resp.hover_tip_dark("右回転 [R]");
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
            let rccw_resp = rccw_resp.hover_tip_dark("左回転 [L]");
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
        let info_resp = info_resp.hover_tip_dark("メタデータ [I / Tab]");
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
            let analysis_resp = analysis_resp.hover_tip_dark("分析ツール [Z]");
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
            let spread_resp = spread_resp.hover_tip_dark("見開き設定 [1-5]");
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
            let resp = resp.hover_tip_dark(tooltip);
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
        let Some((ref text, start_time, duration)) = self.fs_feedback_toast else {
            return;
        };
        let elapsed = start_time.elapsed().as_secs_f32();
        if elapsed > duration {
            self.fs_feedback_toast = None;
            self.fs_feedback_toast_reveal_path = None;
            return;
        }

        // フェードアウト (最後の0.3秒)
        let alpha = if elapsed > duration - 0.3 {
            ((duration - elapsed) / 0.3).clamp(0.0, 1.0)
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

        if let Some(path) = self.fs_feedback_toast_reveal_path.clone() {
            let resp = ui
                .interact(
                    toast_rect,
                    egui::Id::new("feedback_toast_reveal_capture"),
                    egui::Sense::click(),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .hover_tip_dark("クリックで保存ファイルを表示");
            if resp.clicked() {
                crate::capture::reveal_path_async(path);
                self.fs_feedback_toast = None;
                self.fs_feedback_toast_reveal_path = None;
            }
        }

        // フェードアウト中は 30fps で再描画
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }

    /// 画面中央に境界ヒント (最初/最後の項目です… / 次のフォルダが見つかりません…) を描画する。
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
            FsBoundaryHint::NavNoOp { .. } => BOUNDARY_HINT_DURATION,
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
                "最後の項目です",
                vec!["[Home] 最初に戻る", "[Ctrl]+[↓] ツリー順で次へ"],
            ),
            FsBoundaryHint::Edge { at_end: false, .. } => (
                "最初の項目です",
                vec!["[End] 最後に移動", "[Ctrl]+[↑] ツリー順で前へ"],
            ),
            FsBoundaryHint::NoImageFolder { forward: true, .. } => (
                "次のフォルダに画像・動画が見つかりません",
                vec![
                    "[Esc] でサムネイル一覧に戻り",
                    "[Ctrl]+[↓] で空フォルダを越えて移動できます",
                ],
            ),
            FsBoundaryHint::NoImageFolder { forward: false, .. } => (
                "前のフォルダに画像・動画が見つかりません",
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
            FsBoundaryHint::NavNoOp {
                reason: FsNavNoOpReason::LocalFilterActive,
                ..
            } => (
                Self::nav_noop_title(FsNavNoOpReason::LocalFilterActive),
                vec!["現在の一覧フィルタを維持します"],
            ),
            FsBoundaryHint::NavNoOp {
                reason: FsNavNoOpReason::SearchResultList,
                ..
            } => (
                Self::nav_noop_title(FsNavNoOpReason::SearchResultList),
                vec!["結果を開くと検索スコープ内で移動できます"],
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

    pub(crate) fn save_video_frame_to_file(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.capture_pending.is_some() {
            self.show_feedback_toast("キャプチャ保存中です".to_string());
            return;
        }

        let Some((path, target_secs)) = self.fs_video_player(fs_idx).and_then(|player| {
            if player.error().is_some() || player.info().is_none() {
                None
            } else {
                Some((player.path().clone(), player.screenshot_target_secs()))
            }
        }) else {
            self.show_feedback_toast("動画フレームを保存できません".to_string());
            return;
        };

        let output_dir = self.capture_output_dir_path();
        let format = self.settings.capture_format;
        let basename = crate::capture::basename_for_path(&path);
        let (tx, rx) = std::sync::mpsc::channel();
        let worker_path = path.clone();
        let thread = std::thread::Builder::new()
            .name("video-frame-save".into())
            .spawn(move || {
                let result = match crate::video::screenshot::capture_frame(&worker_path, target_secs)
                {
                    Ok(frame) => {
                        crate::logger::log(format!(
                            "video frame captured for save: target={:.3}s decoded_target={:.3}s {}x{} {}",
                            target_secs,
                            frame.target_secs,
                            frame.width,
                            frame.height,
                            worker_path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                        ));
                        crate::capture::save_rgba_unique(
                            &output_dir,
                            &basename,
                            format,
                            frame.width,
                            frame.height,
                            &frame.rgba,
                        )
                    }
                    Err(err) => Err(format!("動画フレーム取得に失敗しました: {err}")),
                };
                let _ = tx.send(result);
            });

        match thread {
            Ok(_) => {
                self.capture_pending = Some(crate::app::CapturePending { rx });
                self.show_feedback_toast(format!("動画フレームを保存中 ({})", format.label()));
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            Err(err) => {
                self.show_feedback_toast(format!("保存 worker を開始できません: {err}"));
            }
        }
    }

    pub(crate) fn toggle_compare_pin_from_current(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.compare_pin_pending.is_some() {
            self.show_feedback_toast("比較画像を準備中です".to_string());
            return;
        }

        if self
            .pinned_compare_slot
            .as_ref()
            .is_some_and(|slot| slot.source_idx == fs_idx)
        {
            self.pinned_compare_slot = None;
            self.compare_view_mode = crate::app::CompareViewMode::Off;
            self.compare_pin_load_pending = None;
            self.compare_prepare_pending = None;
            self.compare_prepared_pair = None;
            self.compare_wipe_dragging = false;
            self.show_feedback_toast("比較画像を解除しました".to_string());
            ctx.request_repaint();
            return;
        }

        let work = match self.prepare_capture_pixel_work(fs_idx) {
            Ok(work) => work,
            Err(err) => {
                self.show_feedback_toast(err);
                return;
            }
        };
        self.start_compare_pin_work(ctx, fs_idx, work);
    }

    pub(crate) fn start_compare_pin_single(&mut self, ctx: &egui::Context, idx: usize) {
        let work = match self.prepare_capture_pixel_job(idx) {
            Ok(job) => crate::capture::CapturePixelWork::Single(job),
            Err(err) => {
                self.show_feedback_toast(err);
                return;
            }
        };
        self.start_compare_pin_work(ctx, idx, work);
    }

    fn start_compare_pin_work(
        &mut self,
        ctx: &egui::Context,
        source_idx: usize,
        work: crate::capture::CapturePixelWork,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("compare-pin".into())
            .spawn(move || {
                let result =
                    crate::capture::run_pixel_work(work).map(|(basename, width, height, rgba)| {
                        crate::app::ComparePinResult {
                            basename,
                            width,
                            height,
                            rgba,
                        }
                    });
                let _ = tx.send(result);
            });

        match thread {
            Ok(_) => {
                self.compare_pin_load_pending = None;
                self.compare_prepare_pending = None;
                self.compare_prepared_pair = None;
                self.compare_pin_pending = Some(crate::app::ComparePinPending { source_idx, rx });
                self.show_feedback_toast("比較画像を準備中".to_string());
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            Err(err) => {
                self.show_feedback_toast(format!("比較 worker を開始できません: {err}"));
            }
        }
    }

    pub(crate) fn toggle_compare_pin_from_grid_selection(&mut self, ctx: &egui::Context) {
        let Some(idx) = self.selected else {
            self.show_feedback_toast("比較画像にする画像を選択してください".to_string());
            return;
        };
        if !matches!(
            self.items.get(idx),
            Some(GridItem::Image(_))
                | Some(GridItem::ZipImage { .. })
                | Some(GridItem::PdfPage { .. })
        ) {
            self.show_feedback_toast("このアイテムは比較画像に設定できません".to_string());
            return;
        }

        if self.compare_pin_pending.is_some() {
            self.show_feedback_toast("比較画像を準備中です".to_string());
            return;
        }

        if self
            .pinned_compare_slot
            .as_ref()
            .is_some_and(|slot| slot.source_idx == idx)
        {
            self.toggle_compare_pin_from_current(ctx, idx);
            return;
        }

        let Some(source_key) = self.metadata_cache_key(idx) else {
            self.show_feedback_toast("このアイテムは比較画像に設定できません".to_string());
            return;
        };

        match self.fs_cache.get(&idx) {
            Some(FsCacheEntry::Static { .. }) | Some(FsCacheEntry::Animated { .. }) => {
                self.start_compare_pin_single(ctx, idx);
            }
            Some(FsCacheEntry::Failed) => {
                self.show_feedback_toast("比較画像を読み込めませんでした".to_string());
            }
            _ => {
                if self.compare_pin_pending.is_some() {
                    self.show_feedback_toast("比較画像を準備中です".to_string());
                    return;
                }
                self.compare_pin_load_pending = Some(crate::app::ComparePinLoadPending {
                    source_idx: idx,
                    source_key,
                });
                if !self.fs_pending.contains_key(&idx) {
                    self.start_fs_load(idx);
                }
                self.show_feedback_toast("比較画像を読み込み中".to_string());
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
    }

    pub(crate) fn toggle_compare_pinned_view(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.pinned_compare_slot.is_none() {
            self.show_feedback_toast("比較画像が未設定です。X で設定してください".to_string());
            return;
        }
        self.compare_view_mode = match self.compare_view_mode {
            crate::app::CompareViewMode::PinnedNormal => crate::app::CompareViewMode::Off,
            crate::app::CompareViewMode::Off => crate::app::CompareViewMode::PinnedNormal,
            crate::app::CompareViewMode::Wipe { .. } => crate::app::CompareViewMode::Off,
            crate::app::CompareViewMode::Diff => crate::app::CompareViewMode::Off,
        };
        self.compare_wipe_dragging = false;
        if matches!(
            self.compare_view_mode,
            crate::app::CompareViewMode::PinnedNormal
        ) {
            self.ensure_compare_prepared_pair(ctx, fs_idx);
        }
        let label = match self.compare_view_mode {
            crate::app::CompareViewMode::PinnedNormal => "[比較: ピン表示]",
            _ => "[比較: 現在表示]",
        };
        self.show_feedback_toast(label.to_string());
    }

    pub(crate) fn toggle_compare_wipe_mode(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.pinned_compare_slot.is_none() {
            self.show_feedback_toast("比較画像が未設定です。X で設定してください".to_string());
            return;
        }
        self.compare_view_mode = match self.compare_view_mode {
            crate::app::CompareViewMode::Wipe { .. } => crate::app::CompareViewMode::Off,
            _ => crate::app::CompareViewMode::Wipe { fraction: 0.5 },
        };
        self.compare_wipe_dragging = false;
        if matches!(
            self.compare_view_mode,
            crate::app::CompareViewMode::Wipe { .. }
        ) {
            self.ensure_compare_prepared_pair(ctx, fs_idx);
        }
        let label = match self.compare_view_mode {
            crate::app::CompareViewMode::Wipe { .. } => "[比較: Wipe]",
            _ => "[比較: Normal]",
        };
        self.show_feedback_toast(label.to_string());
    }

    pub(crate) fn toggle_compare_diff_mode(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.pinned_compare_slot.is_none() {
            self.show_feedback_toast("比較画像が未設定です。X で設定してください".to_string());
            return;
        }
        self.compare_view_mode = match self.compare_view_mode {
            crate::app::CompareViewMode::Diff => crate::app::CompareViewMode::Off,
            _ => crate::app::CompareViewMode::Diff,
        };
        self.compare_wipe_dragging = false;
        if matches!(self.compare_view_mode, crate::app::CompareViewMode::Diff) {
            self.ensure_compare_prepared_pair(ctx, fs_idx);
        }
        let label = match self.compare_view_mode {
            crate::app::CompareViewMode::Diff => "[比較: Diff]",
            _ => "[比較: Normal]",
        };
        self.show_feedback_toast(label.to_string());
    }

    pub(crate) fn save_image_capture_to_file(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.capture_pending.is_some() {
            self.show_feedback_toast("キャプチャ保存中です".to_string());
            return;
        }

        let work = match self.prepare_capture_pixel_work(fs_idx) {
            Ok(work) => work,
            Err(err) => {
                self.show_feedback_toast(err);
                return;
            }
        };
        let format = self.settings.capture_format;
        let output_dir = self.capture_output_dir_path();
        let jpeg_matte =
            crate::capture::JpegMatte::from_fs_transparent_bg_mode(self.fs_transparent_bg_mode);
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("image-capture-save".into())
            .spawn(move || {
                let result = crate::capture::run_pixel_work(work).and_then(
                    |(basename, width, height, rgba)| {
                        crate::capture::save_rgba_unique_with_matte(
                            &output_dir,
                            &basename,
                            format,
                            jpeg_matte,
                            width,
                            height,
                            &rgba,
                        )
                    },
                );
                let _ = tx.send(result);
            });

        match thread {
            Ok(_) => {
                self.capture_pending = Some(crate::app::CapturePending { rx });
                self.show_feedback_toast(format!("キャプチャを保存中 ({})", format.label()));
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            Err(err) => {
                self.show_feedback_toast(format!("保存 worker を開始できません: {err}"));
            }
        }
    }

    pub(crate) fn copy_image_capture_to_clipboard(&mut self, fs_idx: usize) {
        let work = match self.prepare_capture_pixel_work(fs_idx) {
            Ok(work) => work,
            Err(err) => {
                self.show_feedback_toast(err);
                return;
            }
        };
        let clipboard_seq = crate::ui_dialogs::context_menu::reserve_clipboard_write_sequence();
        std::thread::Builder::new()
            .name("image-capture-clipboard".into())
            .spawn(move || match crate::capture::run_pixel_work(work) {
                Ok((_basename, width, height, rgba)) => {
                    crate::ui_dialogs::context_menu::copy_rgba_image_to_clipboard_async_seq(
                        width,
                        height,
                        rgba,
                        clipboard_seq,
                    );
                }
                Err(err) => {
                    crate::logger::log(format!("image capture clipboard failed: {err}"));
                }
            })
            .ok();
        self.show_feedback_toast("キャプチャをクリップボードへコピー中".to_string());
    }

    fn prepare_capture_pixel_work(
        &mut self,
        idx: usize,
    ) -> Result<crate::capture::CapturePixelWork, String> {
        match self.resolve_spread_pair(idx) {
            SpreadPair::Single => self
                .prepare_capture_pixel_job(idx)
                .map(crate::capture::CapturePixelWork::Single),
            SpreadPair::Double { left, right } => {
                let left_job = self.prepare_capture_pixel_job(left)?;
                let right_job = self.prepare_capture_pixel_job(right)?;
                let basename = crate::capture::basename_from_text(&format!(
                    "{}_{}",
                    left_job.basename, right_job.basename
                ));
                Ok(crate::capture::CapturePixelWork::Spread {
                    basename,
                    left: left_job,
                    right: right_job,
                })
            }
        }
    }

    fn prepare_capture_pixel_job(
        &self,
        idx: usize,
    ) -> Result<crate::capture::CapturePixelJob, String> {
        let basename = self
            .capture_basename_for_idx(idx)
            .ok_or_else(|| "このアイテムはキャプチャ保存できません".to_string())?;

        if !self.post_filter_bypassed
            && let Some(FsCacheEntry::Static { pixels, .. }) = self.adjustment_cache.get(&idx)
        {
            return Ok(crate::capture::CapturePixelJob::already_adjusted(
                basename,
                pixels.clone(),
            ));
        }

        let bg = self.effective_upscale_bg_mode();
        let source = if self.ai_upscale_enabled || self.ai_denoise_model.is_some() {
            self.ai_upscale_cache
                .get(&(idx, bg))
                .or_else(|| self.fs_cache.get(&idx))
        } else {
            self.fs_cache.get(&idx)
        };
        let Some(FsCacheEntry::Static { pixels, .. }) = source else {
            if let Some(FsCacheEntry::Animated {
                frame_pixels,
                current_frame,
                ..
            }) = source
            {
                let Some(pixels) = frame_pixels.get(*current_frame) else {
                    return Err("アニメーションフレームを取得できません".to_string());
                };
                return Ok(crate::capture::CapturePixelJob::already_adjusted(
                    basename,
                    pixels.clone(),
                ));
            }
            return Err("画像の読み込み完了後に保存してください".to_string());
        };

        Ok(crate::capture::CapturePixelJob::needs_adjustment(
            basename,
            pixels.clone(),
            self.effective_params(idx).clone(),
        ))
    }

    fn capture_basename_for_idx(&self, idx: usize) -> Option<String> {
        let item = self.items.get(idx)?;
        match item {
            GridItem::Image(path) => Some(crate::capture::basename_for_path(path)),
            GridItem::ZipImage {
                zip_path,
                entry_name,
            } => {
                let zip = crate::capture::basename_for_path(zip_path);
                let entry_name = crate::zip_loader::entry_basename(entry_name);
                let entry = std::path::Path::new(entry_name)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(crate::capture::basename_from_text)
                    .unwrap_or_else(|| "entry".to_string());
                Some(format!("{zip}_{entry}"))
            }
            GridItem::PdfPage {
                pdf_path, page_num, ..
            } => {
                let pdf = crate::capture::basename_for_path(pdf_path);
                Some(format!("{pdf}_p{:04}", page_num + 1))
            }
            _ => None,
        }
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
        // タイル中は seek せずカーソル移動に切り替える。Ctrl 併用時だけ 1 行分移動。
        // ↑↓ は consume せず後段の image arrow_up/down (= 前後ファイル) に流す
        // (= マウスホイールと整合)。Shift+↑↓ だけ動画モードで音量に使う。
        let tile_active_for_keyboard = self.video_tile_mode_active;
        let tile_left_ctrl = if tile_active_for_keyboard {
            ctx.input_mut(|i| {
                if i.consume_key(
                    egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                    egui::Key::ArrowLeft,
                ) {
                    Some(true)
                } else if i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowLeft) {
                    Some(true)
                } else if i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowLeft) {
                    Some(false)
                } else if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) {
                    Some(false)
                } else {
                    None
                }
            })
        } else {
            None
        };
        let tile_right_ctrl = if tile_active_for_keyboard {
            ctx.input_mut(|i| {
                if i.consume_key(
                    egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                    egui::Key::ArrowRight,
                ) {
                    Some(true)
                } else if i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowRight) {
                    Some(true)
                } else if i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowRight) {
                    Some(false)
                } else if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) {
                    Some(false)
                } else {
                    None
                }
            })
        } else {
            None
        };
        let tile_left = tile_left_ctrl.is_some();
        let ctrl_shift_left = tile_left_ctrl.is_none()
            && tile_right_ctrl.is_none()
            && ctx.input_mut(|i| {
                i.consume_key(
                    egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                    egui::Key::ArrowLeft,
                )
            });
        let ctrl_shift_right = tile_left_ctrl.is_none()
            && tile_right_ctrl.is_none()
            && ctx.input_mut(|i| {
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
        let save_frame_key = ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::S));
        // Phase 5.5: S キーでタイルモード トグル (動画モード限定)。画像モードの
        // S (スライドショー) とは handle_video_input 先行 consume で分離する。
        let tile_key = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::S));
        // F キーでフレームレート / Perf オーバーレイのトグル (動画モード限定)。
        // 以前 P を使っていたが、P は「現在フレームをピン留め」に再割り当てしたので
        // 移動した (F = Frames / FPS の mnemonic)。画像モードの F は未使用なので
        // 競合しない。
        let perf_key = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F));
        // P キーで現在再生位置をピン留め (動画モード限定)。グリッドモードの P
        // (folder_thumb_pin toggle) と統一した「P = Pin」の mnemonic。画像モードの
        // ポストフィルタは T に移動済み。
        let pin_key = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::P));
        // 比較ビューは静止画 / ZIP / PDF 限定。動画では passthrough させず silent no-op として消費する。
        let compare_x = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::X));
        let compare_alt_c = ctx.input_mut(|i| i.consume_key(egui::Modifiers::ALT, egui::Key::C));
        let compare_shift_c =
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::C));
        let compare_c = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::C));
        // W キー: 頭出し (= seek to 0 + play)。左手で押しやすく、画像モードでも未使用。
        let rewind_key = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::W));
        // J/K: チャプター・ブックマーク・ピンを 1 本のマーカー列にまとめて前後ジャンプ。
        // 矢印キーは既に固定秒数シークに使っているので別キー。J=前、K=次。
        let prev_marker_key = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::J));
        let next_marker_key = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::K));
        // タイルモード中は ESC でも閉じれるようにする (= 一般的な「全画面モード解除」)。
        // ただし ESC は元々フルスクリーン全体を閉じるキーなので、タイルモード中だけ
        // 横取りする。
        let escape_for_tile = self.video_tile_mode_active
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));

        if shift_enter {
            if let Some(p) = video_path {
                open_external_player(p);
            }
            return;
        }
        if enter && self.video_tile_mode_active {
            self.play_selected_video_tile(ctx, fs_idx);
            return;
        }
        if let Some(ctrl) = tile_left_ctrl.or(tile_right_ctrl) {
            self.handle_video_tile_cursor_key(ctx, fs_idx, ctrl, tile_left);
            return;
        }
        if save_frame_key {
            self.save_video_frame_to_file(ctx, fs_idx);
            return;
        }
        if compare_x || compare_alt_c || compare_shift_c || compare_c {
            return;
        }

        // 先に現在の音量だけ取り出す (player borrow を短く保つ)
        let cur_volume = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => player.volume(),
            _ => return,
        };
        // Phase 7.H: 音量は Shift+↑↓ 限定 (= dB fader key step)。プレーン ↑↓ はファイル移動。
        let new_vol = if shift_up {
            Some(crate::settings::step_video_volume_by_fader_key_step(
                cur_volume, 1,
            ))
        } else if shift_down {
            Some(crate::settings::step_video_volume_by_fader_key_step(
                cur_volume, -1,
            ))
        } else {
            None
        };

        // player に作用させる (借用はこの if-let のスコープ内で完結)
        let mut seek_outcome: Option<crate::video::RelativeSeekOutcome> = None;
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
                seek_outcome = Some(player.seek_relative(-5.0));
            }
            if right {
                seek_outcome = Some(player.seek_relative(5.0));
            }
            if shift_left {
                seek_outcome = Some(player.seek_relative(-1.0));
            }
            if shift_right {
                seek_outcome = Some(player.seek_relative(1.0));
            }
            if ctrl_left {
                seek_outcome = Some(player.seek_relative(-30.0));
            }
            if ctrl_right {
                seek_outcome = Some(player.seek_relative(30.0));
            }
            if let Some(v) = new_vol {
                player.set_volume(v);
            }
        }
        // 先頭 / 末尾に達してシークが発行されなかった場合は境界トーストで通知する
        // (player 借用が終わってから self を触る)。
        match seek_outcome {
            Some(crate::video::RelativeSeekOutcome::AtStart) => {
                self.show_feedback_toast("動画先頭です".to_string());
            }
            Some(crate::video::RelativeSeekOutcome::AtEnd) => {
                self.show_feedback_toast("動画末尾です".to_string());
            }
            Some(crate::video::RelativeSeekOutcome::Seeked) | None => {}
        }

        // 設定への反映 (player 借用は終わっているので self.settings を書き換え可能)。
        // Phase 8 (Codex P3-1): 即時 save 化。グリッド列数や動画タイル列数が
        // 即時保存されるのと挙動を揃え、強制終了時の loss を防ぐ。
        if let Some(v) = new_vol {
            self.settings.video_volume = v;
            self.settings.save();
        }
        if mute_key {
            self.toggle_video_session_mute_for_fs_idx(fs_idx);
        }
        if loop_key {
            // cycle_native_video_loop_common は app/native_video.rs (cfg(windows)) に
            // 定義されているため、非 Windows ビルドではリンクできない。
            // mimageviewer の動画機能自体が Windows 限定 (`pub mod video;` が cfg(windows))
            // なので非 Windows では何もしない。
            #[cfg(windows)]
            self.cycle_native_video_loop_common(ctx, fs_idx);
            #[cfg(not(windows))]
            let _ = (ctx, fs_idx);
        }

        // Phase 5.5: S キーでタイルモード トグル。画面サイズは native presenter の
        // 実クライアントサイズ優先で取得 (取得不可なら content rect にフォールバック)。
        // toggle 内で fs_cache / video_tile_state を借用するので、player 借用後に呼ぶ。
        if tile_key {
            let screen = self.video_tile_layout_size(fs_idx, ctx);
            self.toggle_video_tile_mode(fs_idx, screen);
        }
        if perf_key {
            self.video_perf_overlay_visible = !self.video_perf_overlay_visible;
        }
        if pin_key {
            // P キー: タイルモード中は選択中タイル、それ以外は現在再生位置を
            // ピン留め (= HUD の 📌 ボタンと同等)。
            #[cfg(windows)]
            {
                if self.video_tile_mode_active {
                    self.handle_native_video_set_tile_pin_command(ctx, fs_idx);
                } else {
                    // 現在 PTS を `set_native_video_pin` に渡す (内部で seek thumbnail を
                    // request + nearest 取得 + WebP encode + video_pins DB に書き込み)。
                    // 既に同位置のピンがあれば SQL の ON CONFLICT で pin_pts/thumb_webp を
                    // 上書きするだけなので idempotent。
                    let target = self
                        .fs_video_player(fs_idx)
                        .map(|p| p.position())
                        .unwrap_or(0.0);
                    self.handle_native_video_set_pin_command(ctx, fs_idx, target);
                }
            }
            #[cfg(not(windows))]
            let _ = (ctx, fs_idx);
        }
        if rewind_key && let Some(p) = self.fs_video_player(fs_idx) {
            // `seek(0.0)` は内部で `apply_command(Play)` を発行し autoplay intent を
            // 立てるので、追加 `toggle_play()` は不要 (Codex P2-1 2026-05-17)。
            p.seek(0.0);
        }

        // 何らかの動画ショートカット入力があれば HUD のフェードタイマをリセット
        // (= HUD を再表示)。マウス活動と同様の扱い。
        // J/K: マーカー (チャプター/ブックマーク/ピン) 間の前後ジャンプ。
        // 現在再生位置 ± epsilon を境にした最近接探索で「現在マーカーで足踏み」を防ぐ。
        // J で前のマーカーが見つからないときは動画先頭 (0.0) へ seek (= 既に先頭に
        // 居る場合だけ何もしない、閾値は ALREADY_AT_START_TOL)。K のときは何もしない。
        const ALREADY_AT_START_TOL: f64 = 0.05;
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
            match target {
                Some(m) => {
                    if let Some(p) = self.fs_video_player(fs_idx) {
                        p.seek(m.pts);
                    }
                    // CH/BM ループ中ならマーカージャンプ後に loop_target を更新
                    #[cfg(windows)]
                    self.apply_loop_mode_to_player(fs_idx);
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
                None if !next_marker_key && current > ALREADY_AT_START_TOL => {
                    // J キーで前のマーカーが見つからない (= 最初のマーカー手前または空) かつ
                    // 既に先頭に居なければ動画先頭へ seek。
                    if let Some(p) = self.fs_video_player(fs_idx) {
                        p.seek(0.0);
                    }
                    #[cfg(windows)]
                    self.apply_loop_mode_to_player(fs_idx);
                    self.show_feedback_toast(format!(
                        "{} 動画先頭",
                        crate::ui_helpers::format_hms(0.0)
                    ));
                }
                None => {
                    // K キーでマーカーが無い (= 末尾以降) ケース、
                    // または J キーで既に先頭にいるケースは何もしない。
                }
            }
        }
        // ESC は タイルモード中のみキャッチして close。フルスクリーン解脱は呼び出し側
        // (handle_image_keys 後段) の通常 ESC で扱う。
        if escape_for_tile {
            self.close_video_tile_mode();
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

    /// 動画再生のエラー インジケータを描画する。
    /// playback controls (HUD, seek bar, volume, etc.) と「動画を準備中…」スピナーは
    /// **native presenter overlay** (`src/video/native_presenter/overlay_draw.rs` の
    /// `draw_native_center_status`) が描画する (= native window が egui main の上に
    /// 乗るので、ここで描画しても見えない)。本関数では egui main window 側に
    /// 直接出すべきエラー文言だけを担当する。
    ///
    /// (旧挙動として `!has_texture` で「動画を準備中...」を egui に描いていたが、
    /// native presenter が上に乗ると見えなかったので削除。進捗 HUD の本物は
    /// native presenter 側で `build_preparing_message` を使って描く。)
    pub(crate) fn draw_video_hud(&self, ui: &mut egui::Ui, full_rect: egui::Rect, fs_idx: usize) {
        let has_error = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => player.error().map(|s| s.to_string()),
            _ => return,
        };

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
        }
    }
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
